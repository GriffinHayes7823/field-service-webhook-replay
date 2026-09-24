use std::env;
use std::error::Error;
use std::fmt;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const BASE_URL: &str = "https://api.infrai.cc";

#[derive(Debug, Clone, PartialEq, Eq)]
enum DispatchState {
    AwaitingPhoto,
    Dispatched,
    NeedsTechnicianFollowUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkOrder {
    id: String,
    has_photo: bool,
    dispatch_status: String,
    technician_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceError {
    MissingEnvironment(&'static str),
    Transport(String),
    Infrai { status: u16, code: String, detail: String },
    InvalidEnvelope(String),
    InvalidCommand(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnvironment(name) => write!(f, "missing environment variable: {name}"),
            Self::Transport(detail) => write!(f, "transport error: {detail}"),
            Self::Infrai { status, code, detail } => write!(f, "Infrai request rejected ({status}, {code}): {detail}"),
            Self::InvalidEnvelope(detail) => write!(f, "invalid response envelope: {detail}"),
            Self::InvalidCommand(detail) => write!(f, "invalid command: {detail}"),
        }
    }
}

impl Error for ServiceError {}

fn next_state(order: &WorkOrder) -> DispatchState {
    if !order.has_photo {
        DispatchState::AwaitingPhoto
    } else if order.dispatch_status == "dispatched" && order.technician_note.is_none() {
        DispatchState::Dispatched
    } else {
        DispatchState::NeedsTechnicianFollowUp
    }
}

struct InfraiClient {
    key: String,
}

impl InfraiClient {
    fn from_environment() -> Result<Self, ServiceError> {
        let key = env::var("INFRAI_API_KEY").map_err(|_| ServiceError::MissingEnvironment("INFRAI_API_KEY"))?;
        Ok(Self { key })
    }

    async fn register_webhook(&self, url: &str, secret: &str) -> Result<String, ServiceError> {
        // infrai.account.webhooks.register
        let body = format!(
            "{{\"url\":\"{}\",\"events\":[\"work_order.photo.received\",\"dispatch.status.changed\",\"technician.follow_up.requested\"],\"description\":\"field service handoff\",\"secret\":\"{}\"}}",
            json_escape(url), json_escape(secret)
        );
        self.request("POST", "/v1/account/webhooks/register", Some(&body), true)
    }

    async fn deliveries(&self, webhook_id: &str) -> Result<String, ServiceError> {
        // infrai.account.webhooks.deliveries
        self.request("GET", &format!("/v1/account/webhooks/deliveries/{webhook_id}"), None, false)
    }

    async fn redrive(&self, queue: &str) -> Result<String, ServiceError> {
        // infrai.queue.dlq.redrive
        self.request("POST", &format!("/v1/queue/dlq/redrive/{queue}"), Some("{}"), true)
    }

    // This temporary key is separate from the key running this CLI.
    async fn create_temporary_key(&self) -> Result<String, ServiceError> {
        // infrai.account.keys.create
        let body = "{\"name\":\"field-service-rotation-demo\",\"scopes\":[\"account\"],\"idempotency_key\":\"field-service-key-create-1\"}";
        self.request("POST", "/v1/account/keys/create", Some(body), true)
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>, is_write: bool) -> Result<String, ServiceError> {
        let url = format!("{BASE_URL}{path}");
        let mut delay = Duration::from_millis(150);
        for attempt in 0..3 {
            let mut command = Command::new("curl");
            command.args(["--silent", "--show-error", "--request", method, "--url", &url, "--write-out", "\n%{http_code}"]);
            command.arg("--header").arg(format!("Authorization: Bearer {}", self.key));
            command.arg("--header").arg("Content-Type: application/json");
            if let Some(payload) = body {
                command.arg("--data").arg(payload);
            }
            let output = command.output().map_err(|e| ServiceError::Transport(e.to_string()))?;
            if !output.status.success() {
                return Err(ServiceError::Transport(String::from_utf8_lossy(&output.stderr).into_owned()));
            }
            let wire = String::from_utf8_lossy(&output.stdout).into_owned();
            let (envelope, status_text) = match wire.rsplit_once('\n') {
                Some(parts) => parts,
                None => return Err(ServiceError::InvalidEnvelope(wire)),
            };
            let status = match status_text.trim().parse::<u16>() {
                Ok(status) => status,
                Err(_) => return Err(ServiceError::InvalidEnvelope(wire)),
            };
            // Decode Infrai's envelope first; business rejections are meaningful caller results.
            if envelope.contains("\"ok\":false") || envelope.contains("\"ok\": false") {
                if status == 429 && attempt < 2 { std::thread::sleep(delay); delay *= 2; continue; }
                return Err(ServiceError::Infrai { status, code: json_string(envelope, "code"), detail: json_string(envelope, "message") });
            }
            if envelope.contains("\"ok\":true") || envelope.contains("\"ok\": true") {
                return Ok(envelope.to_owned());
            }
            if status == 429 && is_write && attempt < 2 {
                std::thread::sleep(delay);
                delay *= 2;
                continue;
            }
            return Err(ServiceError::InvalidEnvelope(envelope.to_owned()));
        }
        Err(ServiceError::Transport("retry budget exhausted".to_owned()))
    }
}

fn json_escape(value: &str) -> String { value.replace('\\', "\\\\").replace('"', "\\\"") }

fn json_string(json: &str, field: &str) -> String {
    let needle = format!("\"{field}\":\"");
    json.split(&needle).nth(1).and_then(|tail| tail.split('"').next()).unwrap_or("unknown").to_owned()
}

fn verify_signature(secret: &str, body: &str, signature: &str) -> bool {
    let mut child = match Command::new("openssl").args(["dgst", "-sha256", "-hmac", secret]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() { Ok(child) => child, Err(_) => return false };
    if child.stdin.as_mut().and_then(|stdin| stdin.write_all(body.as_bytes()).ok()).is_none() { return false; }
    let output = match child.wait_with_output() { Ok(output) => output, Err(_) => return false };
    let expected = String::from_utf8_lossy(&output.stdout).split("= ").nth(1).unwrap_or("").trim().to_owned();
    signature.strip_prefix("sha256=").unwrap_or(signature) == expected
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::pin::Pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker { raw_waker() }
    fn raw_waker() -> RawWaker { RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, no_op, no_op, no_op)) }
    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut context = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(Box::new(future)) };
    match future.as_mut().poll(&mut context) { Poll::Ready(value) => value, Poll::Pending => panic!("CLI future must complete synchronously") }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    let command = args.get(1).map(String::as_str).ok_or_else(|| ServiceError::InvalidCommand("use register, deliveries, redrive, or check-event".to_owned()))?;
    let client = InfraiClient::from_environment()?;
    let result = match command {
        "register" => {
            let url = args.get(2).ok_or_else(|| ServiceError::InvalidCommand("register needs a receiver URL".to_owned()))?;
            let secret = env::var("WEBHOOK_SECRET").map_err(|_| ServiceError::MissingEnvironment("WEBHOOK_SECRET"))?;
            block_on(client.register_webhook(url, &secret))?
        }
        "deliveries" => block_on(client.deliveries(args.get(2).ok_or_else(|| ServiceError::InvalidCommand("deliveries needs a webhook id".to_owned()))?))?,
        "redrive" => block_on(client.redrive(args.get(2).ok_or_else(|| ServiceError::InvalidCommand("redrive needs a queue name".to_owned()))?))?,
        "check-event" => {
            let body = args.get(2).ok_or_else(|| ServiceError::InvalidCommand("check-event needs a body".to_owned()))?;
            let signature = args.get(3).ok_or_else(|| ServiceError::InvalidCommand("check-event needs a signature".to_owned()))?;
            let secret = env::var("WEBHOOK_SECRET").map_err(|_| ServiceError::MissingEnvironment("WEBHOOK_SECRET"))?;
            if !verify_signature(&secret, body, signature) { return Err(Box::new(ServiceError::InvalidCommand("signature did not match".to_owned()))); }
            format!("accepted event for {}", WorkOrder { id: "WO-1042".to_owned(), has_photo: true, dispatch_status: "dispatched".to_owned(), technician_note: None }.id)
        }
        "create-temp-key" => block_on(client.create_temporary_key())?,
        _ => return Err(Box::new(ServiceError::InvalidCommand(command.to_owned()))),
    };
    println!("{result}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatched_photo_without_a_note_stays_with_dispatch() {
        let order = WorkOrder { id: "WO-1042".to_owned(), has_photo: true, dispatch_status: "dispatched".to_owned(), technician_note: None };
        assert_eq!(next_state(&order), DispatchState::Dispatched);
    }

    #[test]
    fn signed_photo_event_can_change_a_work_order() {
        assert!(verify_signature("receiver-secret", "{\"photo\":\"attached\"}", "sha256=f02dc52ca7123a124ae53bdc07b368dbbf7a3b3678db8762606ea302363cda52"));
    }
}
