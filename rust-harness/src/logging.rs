use std::{
    backtrace::Backtrace,
    collections::BTreeMap,
    env, fs,
    panic::{self, PanicInfo},
    path::Path,
    sync::{Once, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::json;

const DEFAULT_LOG_PATH: &str = "/tmp/continuum-rust-harness.jsonl";
const DEFAULT_LOG_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_SLOW_OP_MS: u64 = 25;
const DEFAULT_SLOW_VIEW_BUILD_MS: u64 = 10;

#[derive(Clone)]
struct LoggingConfig {
    log_path: String,
    log_max_bytes: u64,
    slow_op_ms: u64,
    slow_view_build_ms: u64,
}

static CONFIG: OnceLock<LoggingConfig> = OnceLock::new();
static INIT_PANIC_HOOK: Once = Once::new();

pub fn init_native_logging() {
    let _ = config();
    INIT_PANIC_HOOK.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            log_panic(info);
            default_hook(info);
        }));
    });
}

pub fn log_call_outcome(
    component: &str,
    duration: Duration,
    status: &str,
    error_message: Option<&str>,
    mut fields: BTreeMap<String, String>,
) {
    let cfg = config();
    let duration_ms = duration.as_secs_f64() * 1_000.0;
    if status == "ok" && duration_ms < cfg.slow_op_ms as f64 {
        return;
    }

    fields.insert("status".to_string(), status.to_string());
    fields.insert("duration_ms".to_string(), format!("{duration_ms:.3}"));
    if let Some(message) = error_message {
        fields.insert("error".to_string(), truncate(message, 4_000));
    }

    log_record(
        if status == "ok" { "warn" } else { "error" },
        component,
        Some(if status == "ok" {
            "slow native call"
        } else {
            "native call failed"
        }),
        fields,
    );
}

pub fn log_view_build(
    view: &str,
    total: Duration,
    projection: Duration,
    prune: Duration,
    snapshot: Duration,
    markets: usize,
    users: usize,
    accounts: usize,
) {
    let cfg = config();
    let total_ms = total.as_secs_f64() * 1_000.0;
    if total_ms < cfg.slow_view_build_ms as f64 {
        return;
    }

    let mut fields = BTreeMap::new();
    fields.insert("view".to_string(), view.to_string());
    fields.insert("duration_ms".to_string(), format!("{total_ms:.3}"));
    fields.insert(
        "projection_ms".to_string(),
        format!("{:.3}", projection.as_secs_f64() * 1_000.0),
    );
    fields.insert(
        "prune_ms".to_string(),
        format!("{:.3}", prune.as_secs_f64() * 1_000.0),
    );
    fields.insert(
        "snapshot_ms".to_string(),
        format!("{:.3}", snapshot.as_secs_f64() * 1_000.0),
    );
    fields.insert("markets".to_string(), markets.to_string());
    fields.insert("users".to_string(), users.to_string());
    fields.insert("accounts".to_string(), accounts.to_string());
    log_record(
        "warn",
        "replay.build_view_state",
        Some("slow view build"),
        fields,
    );
}

pub fn log_error(component: &str, message: &str, fields: BTreeMap<String, String>) {
    log_record("error", component, Some(message), fields);
}

fn log_panic(info: &PanicInfo<'_>) {
    let mut fields = BTreeMap::new();
    fields.insert("thread".to_string(), current_thread_name());
    fields.insert("payload".to_string(), panic_message(info));
    if let Some(location) = info.location() {
        fields.insert("file".to_string(), location.file().to_string());
        fields.insert("line".to_string(), location.line().to_string());
        fields.insert("column".to_string(), location.column().to_string());
    }
    fields.insert(
        "backtrace".to_string(),
        truncate(&Backtrace::force_capture().to_string(), 16_000),
    );
    log_record("error", "rust_panic", Some("panic hook triggered"), fields);
}

fn config() -> &'static LoggingConfig {
    CONFIG.get_or_init(|| LoggingConfig {
        log_path: env::var("CONTINUUM_HARNESS_RUST_LOG_PATH")
            .unwrap_or_else(|_| DEFAULT_LOG_PATH.to_string()),
        log_max_bytes: env_u64(
            "CONTINUUM_HARNESS_RUST_LOG_MAX_BYTES",
            DEFAULT_LOG_MAX_BYTES,
        ),
        slow_op_ms: env_u64("CONTINUUM_HARNESS_RUST_SLOW_OP_MS", DEFAULT_SLOW_OP_MS),
        slow_view_build_ms: env_u64(
            "CONTINUUM_HARNESS_RUST_SLOW_VIEW_BUILD_MS",
            DEFAULT_SLOW_VIEW_BUILD_MS,
        ),
    })
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn log_record(
    level: &str,
    component: &str,
    message: Option<&str>,
    fields: BTreeMap<String, String>,
) {
    let cfg = config();
    let record = json!({
        "ts_ms": now_ts_ms(),
        "level": level,
        "component": component,
        "message": message.unwrap_or(""),
        "fields": fields,
    });
    let line = format!("{record}\n");

    if let Err(err) = append_with_rotation(&cfg.log_path, &line, cfg.log_max_bytes) {
        eprintln!(
            "[rust-harness:{component}] failed to write structured log: {}",
            err
        );
        return;
    }

    if level != "warn" {
        eprintln!("[rust-harness:{component}] {}", message.unwrap_or_default());
    }
}

fn append_with_rotation(path: &str, line: &str, max_bytes: u64) -> std::io::Result<()> {
    ensure_dir(path)?;
    let current_size = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let line_bytes = line.len() as u64;
    if max_bytes > 0 && current_size.saturating_add(line_bytes) > max_bytes {
        let rotated_path = format!("{path}.1");
        if Path::new(&rotated_path).exists() {
            let _ = fs::remove_file(&rotated_path);
        }
        if Path::new(path).exists() {
            fs::rename(path, rotated_path)?;
        }
    }
    let mut content = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    std::io::Write::write_all(&mut content, line.as_bytes())
}

fn ensure_dir(path: &str) -> std::io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        value.to_string()
    } else {
        format!("{}…", &value[..limit])
    }
}

fn panic_message(info: &PanicInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn current_thread_name() -> String {
    std::thread::current()
        .name()
        .map(|name| name.to_string())
        .unwrap_or_else(|| "unnamed".to_string())
}

fn now_ts_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
