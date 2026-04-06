# Debugger Cron — Auto-healing via Claude Code

## Overview

A cron job (`watch_the_watcher.sh`) monitors the service-monitor watcher. When a service fails repeatedly despite the watcher's own auto-heal attempts, it spawns a headless Claude Code instance to diagnose, write a bug report, restart, and verify.

## Cron Entry

```
*/2 * * * * /home/hetalkenaudekar/stagin4/scripts/watch_the_watcher.sh >> /home/hetalkenaudekar/stagin4/claude_watcher_logs/cron.log 2>&1
```

Runs every 2 minutes as user `dm`.

## How to Pause / Resume

```bash
# Pause — comment out the cron line
crontab -l | sed 's|^\(\*/2.*watch_the_watcher\)|#\1|' | crontab -

# Resume — uncomment
crontab -l | sed 's|^#\(\*/2.*watch_the_watcher\)|\1|' | crontab -

# Verify
crontab -l
```

## Script

`/home/hetalkenaudekar/stagin4/scripts/watch_the_watcher.sh`

## Detection Logic

1. **Service-monitor alive?** — checks `screen -ls` for `.service-monitor`. If dead, restarts it and exits.
2. **Repeated ALERTs?** — parses `service_monitor.log` for ALERT lines in the last 240 seconds. If the same service has 3+ ALERTs (meaning auto-heal keeps failing), triggers Claude.

### Alert-to-service mapping

| Alert pattern               | Service key    |
|-----------------------------|----------------|
| `Harness DOWN`              | harness        |
| `Relayer/cranker DOWN`      | relayer        |
| `Execution queue STALLED`   | relayer        |
| `Event queue HIGH`          | event-cranker  |
| `Bots LOW`                  | quoters        |

## Claude Code Invocation

```bash
claude -p "<prompt>" \
  --permission-mode bypassPermissions \
  --add-dir /home/hetalkenaudekar/stagin4 \
  --max-budget-usd 1 \
  --no-session-persistence
```

- **Non-interactive**: `-p` (print mode, no TTY needed)
- **Unattended**: `--permission-mode bypassPermissions` (reads logs, runs restarts without prompting)
- **Cost cap**: $1 USD per invocation
- **No persistence**: session not saved to disk

## Claude Code Deliverables (per invocation)

1. **Diagnose** — reads service logs, journalctl, service_monitor.log, divergence_monitor.log
2. **Bug report** — appends timestamped incident to `/home/hetalkenaudekar/stagin4/bugreport.md`
3. **Restart** — executes appropriate restart (systemctl for systemd services, screen kill+relaunch for screen services)
4. **Verify** — waits 15s, checks health endpoint / process count, marks resolved or unresolved
5. **Exit** — shuts off when done

## Monitored Services

| Service              | Type    | Unit / Screen                          | Health Check                           | Restart Command |
|----------------------|---------|----------------------------------------|----------------------------------------|-----------------|
| Harness              | systemd | `stagin4-devnet-harness.service`       | `curl http://127.0.0.1:9091/livez`     | `sudo systemctl restart stagin4-devnet-harness.service` |
| Relayer / Exec Crank | systemd | `stagin4-devnet-relayer.service`       | `curl http://127.0.0.1:9093/healthz`   | `sudo systemctl restart stagin4-devnet-relayer.service` |
| Event Queue Cranker  | screen  | `event-cranker`                        | event_q count in service_monitor.log   | Kill screen + relaunch `run_event_cranker.sh` |
| Quoter Bots          | screen  | `quoters`                              | `ps aux \| grep random-sol-usdc-quoter` | Kill screen + pkill bots + relaunch `run_persistent_quoters.sh` |
| Service Monitor      | screen  | `service-monitor`                      | `screen -ls \| grep service-monitor`   | Relaunch `run_service_monitor.sh` in screen |
| Divergence Monitor   | screen  | `divergence-monitor`                   | Informational only                     | N/A |

## Key File Paths

| What                    | Path |
|-------------------------|------|
| Meta-watcher script     | `/home/hetalkenaudekar/stagin4/scripts/watch_the_watcher.sh` |
| Service monitor script  | `/home/hetalkenaudekar/stagin4/scripts/run_service_monitor.sh` |
| Service monitor log     | `/home/hetalkenaudekar/stagin4/service_monitor.log` |
| Divergence monitor log  | `/home/hetalkenaudekar/stagin4/divergence_monitor.log` |
| Bug reports             | `/home/hetalkenaudekar/stagin4/bugreport.md` |
| Claude invocation logs  | `/home/hetalkenaudekar/stagin4/claude_watcher_logs/claude_<timestamp>.log` |
| Cron output log         | `/home/hetalkenaudekar/stagin4/claude_watcher_logs/cron.log` |
| Lock file               | `/tmp/watch_the_watcher.lock` |
| Harness log             | `/home/hetalkenaudekar/stagin4/mng-v4/.devnet/logs/continuum-harness.log` |
| Relayer log             | `/home/hetalkenaudekar/stagin4/mng-v4/.devnet/logs/ctm-relayer.log` |
| Quoter tier logs        | `/home/hetalkenaudekar/stagin4/mng-v4/.devnet/logs/persistent-tier{1..5}.log` |
| Env file                | `/home/hetalkenaudekar/stagin4/mng-v4/.devnet/systemd/devnet-stack.env` |

## Safeguards

- **Lock file** (`/tmp/watch_the_watcher.lock`) prevents concurrent Claude instances
- **Cost cap** ($1/invocation) limits API spend
- **No code changes** — Claude prompt forbids modifying application code, only reads logs and restarts services
- **Recovery check** — Claude checks live status before restarting; if the service already recovered, it logs the incident but skips the restart
