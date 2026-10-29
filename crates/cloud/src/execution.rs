//! Luau execution: a task on a place version (or the head), its state, its results and its logs.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{Call, Client, Error, ErrorKind, code};

/// Open Cloud's limits on a task's script and its timeout.
pub const MAX_SCRIPT: usize = 4 << 20;
pub const MAX_TASK_TIMEOUT: Duration = Duration::from_secs(300);

const START: &str = "start Luau execution task";
const GET: &str = "get Luau execution task";
const LOGS: &str = "list Luau execution logs";

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// `universes/…/places/…/versions/…/luau-execution-sessions/…/tasks/…`, validated.
    pub path: String,
    /// The version that ran, from the path: it names one even for a task started at the head.
    pub version: u64,
    /// QUEUED, PROCESSING, COMPLETE, FAILED or CANCELLED.
    pub state: String,
    pub error: Option<(String, String)>,
    /// One raw JSON value per value the script returned.
    pub results: Vec<Value>,
}

impl Task {
    pub fn terminal(&self) -> bool {
        matches!(self.state.as_str(), "COMPLETE" | "FAILED" | "CANCELLED")
    }
}

impl Client {
    /// Starts `script` on a saved version, or on the current head for `None`. Not idempotent: an ambiguous failure means
    /// the task may have started.
    pub fn start_task(
        &self,
        universe: u64,
        place: u64,
        version: Option<u64>,
        script: &str,
        timeout: Duration,
    ) -> Result<Task, Error> {
        let at = version.map_or(String::new(), |v| format!("/versions/{v}"));
        let path = format!("cloud/v2/universes/{universe}/places/{place}{at}/luau-execution-session-tasks");
        let body = json!({"script": script, "timeout": duration(timeout)}).to_string();
        let call = Call {
            op: START,
            method: "POST",
            path,
            body: Some(("application/json", body.as_bytes())),
            create: true,
            long: false,
        };
        let invalid = if universe == 0 || place == 0 || version == Some(0) {
            Some(format!("universe {universe}, place {place} and version {version:?} must be positive"))
        } else if script.is_empty() || script.len() > MAX_SCRIPT {
            Some(format!("the script is {} bytes; Open Cloud takes 1 to {MAX_SCRIPT}", script.len()))
        } else if timeout.is_zero() || timeout > MAX_TASK_TIMEOUT {
            Some(format!("the timeout {timeout:?} must be more than 0 and at most 300 s"))
        } else {
            None
        };
        if let Some(message) = invalid {
            return Err(self.error(&call, ErrorKind::Local, message));
        }
        let v = self.json(&call)?;
        let task = self.task(&call, &v, None)?;
        // a task on another place or version would run, and be mapped as, a build it is not
        if !task.path.starts_with(&format!("universes/{universe}/places/{place}/"))
            || version.is_some_and(|v| v != task.version)
        {
            let message = format!("asked for place {place} at {version:?} and Roblox started {}", task.path);
            return Err(self.error(&call, ErrorKind::Decode, message));
        }
        Ok(task)
    }

    /// Polls a task every 250 ms doubling to 500 ms, back to 250 ms whenever its state changes (a short body is seen
    /// promptly once it starts), until it ends or `deadline` passes; a task that has not ended is returned as it is.
    /// `on_state` hears each new state.
    pub fn wait_task(&self, path: &str, deadline: Instant, on_state: &mut dyn FnMut(&str)) -> Result<Task, Error> {
        let call =
            Call { op: GET, method: "GET", path: format!("cloud/v2/{path}"), body: None, create: false, long: false };
        if task_path(path) != Some(path) {
            return Err(self.error(&call, ErrorKind::Local, format!("{path:?} is not a task path")));
        }
        let mut interval = Duration::from_millis(250);
        let mut last = String::new();
        loop {
            let v = self.json(&call)?;
            let task = self.task(&call, &v, Some(path))?;
            if task.state != last {
                on_state(&task.state);
                last = task.state.clone();
                interval = Duration::from_millis(250);
            }
            if task.terminal() || self.now() + interval > deadline {
                return Ok(task);
            }
            self.sleep(interval);
            interval = (interval * 2).min(Duration::from_millis(500));
        }
    }

    /// Every FLAT log line of a task, page after page. A page that fails after the first, or the deadline passing,
    /// ends the lines with a note saying so: logs are best-effort and the earlier pages are worth keeping.
    pub fn task_logs(&self, path: &str, deadline: Instant) -> Result<Vec<String>, Error> {
        if task_path(path) != Some(path) {
            let call = Call {
                op: LOGS,
                method: "GET",
                path: format!("cloud/v2/{path}/logs"),
                body: None,
                create: false,
                long: false,
            };
            return Err(self.error(&call, ErrorKind::Local, format!("{path:?} is not a task path")));
        }
        let mut lines = Vec::new();
        let mut tokens: Vec<String> = Vec::new();
        loop {
            let mut query = format!("cloud/v2/{path}/logs?maxPageSize=1000&view=FLAT");
            if let Some(token) = tokens.last() {
                query += "&pageToken=";
                query += &escape(token);
            }
            let call = Call { op: LOGS, method: "GET", path: query, body: None, create: false, long: false };
            let v = match self.json(&call) {
                Ok(v) => v,
                Err(e) if !tokens.is_empty() => {
                    lines.push(format!("(opossum: the logs stop here, the next page failed: {e})"));
                    return Ok(lines);
                }
                Err(e) => return Err(e),
            };
            for chunk in v["luauExecutionSessionTaskLogs"].as_array().into_iter().flatten() {
                lines.extend(
                    chunk["messages"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from),
                );
            }
            let next = v["nextPageToken"].as_str().unwrap_or("");
            if next.is_empty() {
                return Ok(lines);
            }
            if tokens.iter().any(|t| t == next) {
                return Err(self.error(&call, ErrorKind::Decode, format!("repeated page token {next:?}")));
            }
            if self.now() >= deadline {
                lines.push("(opossum: the logs stop here, reading them took too long)".into());
                return Ok(lines);
            }
            tokens.push(next.into());
        }
    }

    /// A task from a response. Its path must be a task path, and the one asked for when polling (an empty one is).
    fn task(&self, call: &Call, v: &Value, asked: Option<&str>) -> Result<Task, Error> {
        let got = v["path"].as_str().unwrap_or("");
        let path = match (task_path(got), asked) {
            (None, Some(asked)) if got.is_empty() => asked,
            (Some(path), None) => path,
            (Some(path), Some(asked)) if path == asked => path,
            _ => {
                return Err(self.error(
                    call,
                    ErrorKind::Decode,
                    format!("the task path {got:?} is not the one expected"),
                ));
            }
        };
        let mut segments = path.split('/');
        let version = segments.by_ref().find(|s| *s == "versions").and_then(|_| segments.next()?.parse().ok());
        let error = &v["error"];
        Ok(Task {
            path: path.into(),
            version: version.unwrap_or(0),
            state: v["state"].as_str().unwrap_or("").into(),
            error: (!error.is_null()).then(|| (code(&error["code"]), error["message"].as_str().unwrap_or("").into())),
            results: v["output"]["results"].as_array().cloned().unwrap_or_default(),
        })
    }
}

/// A task path Roblox returned, trimmed of one leading `/`, if it is one: under `universes/`, with no backslash, query,
/// fragment, whitespace or empty, `.` or `..` segment. A response can then never steer a GET carrying the key.
fn task_path(path: &str) -> Option<&str> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let clean = path.starts_with("universes/")
        && !path.contains(['\\', '?', '#'])
        && !path.contains(char::is_whitespace)
        && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    clean.then_some(path)
}

/// A protobuf JSON duration: whole seconds, or the fewest of 3, 6 or 9 fractional digits that are exact.
fn duration(d: Duration) -> String {
    let (s, n) = (d.as_secs(), d.subsec_nanos());
    if n == 0 {
        format!("{s}s")
    } else if n % 1_000_000 == 0 {
        format!("{s}.{:03}s", n / 1_000_000)
    } else if n % 1000 == 0 {
        format!("{s}.{:06}s", n / 1000)
    } else {
        format!("{s}.{n:09}s")
    }
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{Mock, Reply};
    use crate::{Clock, Fake};
    use std::sync::Arc;

    const TASK: &str = "universes/12/places/34/versions/56/luau-execution-sessions/s/tasks/t";

    fn client(base: &str) -> (Client, Arc<Fake>) {
        let fake = Fake::new();
        (Client::new("test-key", base, Clock::Fake(fake.clone())).unwrap(), fake)
    }

    #[test]
    fn durations_are_protobuf_json() {
        let cases = [
            (300_000_000_000, "300s"),
            (1_500_000_000, "1.500s"),
            (1_000_234_000, "1.000234s"),
            (1_000_000_234, "1.000000234s"),
        ];
        for (nanos, want) in cases {
            assert_eq!(duration(Duration::from_nanos(nanos)), want);
        }
    }

    #[test]
    fn a_task_starts_at_a_version_or_the_head_and_names_the_version_that_ran() {
        let mock = Mock::start();
        let queued = json!({"path": TASK, "state": "QUEUED"});
        mock.on("POST", "cloud/v2/universes/12/places/34/", Reply::json(200, &queued));
        mock.on("POST", "cloud/v2/universes/12/places/34/", Reply::json(200, &queued));
        let (c, _) = client(&mock.base);
        let task = c.start_task(12, 34, Some(56), "return 1", Duration::from_millis(1500)).unwrap();
        assert_eq!((task.path.as_str(), task.version, task.state.as_str()), (TASK, 56, "QUEUED"));
        assert_eq!(c.start_task(12, 34, None, "return 1", MAX_TASK_TIMEOUT).unwrap().version, 56);
        let r = mock.requests();
        assert_eq!(r[0].path, "cloud/v2/universes/12/places/34/versions/56/luau-execution-session-tasks");
        assert_eq!(r[1].path, "cloud/v2/universes/12/places/34/luau-execution-session-tasks");
        assert_eq!(r[0].json(), json!({"script": "return 1", "timeout": "1.500s"}));
        assert_eq!(r[1].json(), json!({"script": "return 1", "timeout": "300s"}));
        assert_eq!(r[0].header("content-type"), Some("application/json"));
        mock.on("POST", "cloud/v2/universes/12/places/34/", Reply::json(200, &queued));
        assert_eq!(c.start_task(12, 34, Some(57), "return 1", MAX_TASK_TIMEOUT).unwrap_err().kind, ErrorKind::Decode);
    }

    #[test]
    fn a_response_cannot_steer_the_key() {
        let mock = Mock::start();
        for path in
            ["universes/1/../../x", "https://evil.com/universes/1", "universes/1/t?x=1", "universes//1", "other/1"]
        {
            mock.on("POST", "cloud/v2", Reply::json(200, &json!({"path": path, "state": "QUEUED"})));
        }
        let (c, _) = client(&mock.base);
        for _ in 0..5 {
            assert_eq!(c.start_task(1, 2, None, "x", MAX_TASK_TIMEOUT).unwrap_err().kind, ErrorKind::Decode);
        }
        let no = &mut |_: &str| {};
        assert_eq!(c.wait_task("universes/1/../x", c.now(), no).unwrap_err().kind, ErrorKind::Local);
        assert_eq!(c.task_logs("/etc/passwd", c.now()).unwrap_err().kind, ErrorKind::Local);
        let bad = [
            (0, 1, None, "x", MAX_TASK_TIMEOUT),
            (1, 1, Some(0), "x", MAX_TASK_TIMEOUT),
            (1, 1, None, "", MAX_TASK_TIMEOUT),
        ];
        for (u, p, v, script, timeout) in bad {
            assert_eq!(c.start_task(u, p, v, script, timeout).unwrap_err().kind, ErrorKind::Local);
        }
        for timeout in [Duration::ZERO, MAX_TASK_TIMEOUT + Duration::from_millis(1)] {
            assert_eq!(c.start_task(1, 1, None, "x", timeout).unwrap_err().kind, ErrorKind::Local);
        }
        assert_eq!(mock.requests().len(), 5);
    }

    #[test]
    fn polling_resets_to_250_ms_on_each_new_state() {
        let mock = Mock::start();
        let states = ["QUEUED", "QUEUED", "QUEUED", "QUEUED", "PROCESSING", "PROCESSING"];
        for state in states {
            mock.on("GET", &format!("cloud/v2/{TASK}"), Reply::json(200, &json!({"path": TASK, "state": state})));
        }
        let failed = json!({"state": "FAILED", "error": {"code": "SCRIPT_ERROR", "message": "boom"}});
        mock.on("GET", &format!("cloud/v2/{TASK}"), Reply::json(200, &failed));
        let (c, fake) = client(&mock.base);
        let mut seen = Vec::new();
        let task = c.wait_task(TASK, c.now() + Duration::from_secs(60), &mut |s| seen.push(s.to_string())).unwrap();
        let ms: Vec<u128> = fake.slept().iter().map(Duration::as_millis).collect();
        assert_eq!(ms, [250, 500, 500, 500, 250, 500]);
        assert_eq!(seen, ["QUEUED", "PROCESSING", "FAILED"]);
        assert_eq!(task.error, Some(("SCRIPT_ERROR".into(), "boom".into())));
        assert_eq!(task.version, 56);
    }

    #[test]
    fn logs_page_until_the_last_token_and_refuse_a_repeat() {
        let mock = Mock::start();
        let logs = |lines: &[&str], next: &str| json!({"luauExecutionSessionTaskLogs": [{"path": TASK, "messages": lines}], "nextPageToken": next});
        let logs_path = format!("cloud/v2/{TASK}/logs");
        mock.on("GET", &logs_path, Reply::json(200, &logs(&["one", "two"], "a/b=")));
        mock.on("GET", &logs_path, Reply::json(200, &logs(&["three"], "")));
        let (c, _) = client(&mock.base);
        let deadline = c.now() + Duration::from_secs(30);
        assert_eq!(c.task_logs(TASK, deadline).unwrap(), ["one", "two", "three"]);
        let r = mock.requests();
        assert_eq!(r[0].query, "maxPageSize=1000&view=FLAT");
        assert_eq!(r[1].query, "maxPageSize=1000&view=FLAT&pageToken=a%2Fb%3D");

        mock.on("GET", &logs_path, Reply::json(200, &logs(&["x"], "t")));
        mock.on("GET", &logs_path, Reply::json(200, &logs(&["y"], "t")));
        assert!(c.task_logs(TASK, deadline).unwrap_err().message.contains("repeated page token"));
    }
}
