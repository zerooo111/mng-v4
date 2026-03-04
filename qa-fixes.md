# QA Fixes Log

## 2026-03-04

### 1) Local startup false-negative on harness readiness
- Symptom:
  - `./startup_local.sh start` failed with `Port 9091 did not start listening` while the harness process became healthy shortly afterward.
- Root cause:
  - Startup readiness waits were hardcoded to 60 tries/seconds, which is too short for cold local boots in this environment.
- Fix:
  - Added configurable `STARTUP_WAIT_TRIES` (default `180`) and applied it to RPC/HTTP/port/harness readiness checks in `startup_local.sh`.
- Validation:
  - Pending in next restart cycle (to be re-verified in subsequent QA steps).

