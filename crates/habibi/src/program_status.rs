//! OSC 7501 Program Status Protocol (revision 0.4).
//! Fixed messages only: no prompts, credentials, paths, arguments, or provider errors.
use base64::{Engine, engine::general_purpose::STANDARD};
use libhabibi::Event;
use std::io::{self, IsTerminal, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Idle,
    Thinking,
    Responding,
    Tool,
    Compacting,
    Permission,
    Auth,
    Done,
    Error,
}

impl Status {
    fn sequence(self) -> String {
        let (state, kind, message) = match self {
            Self::Idle => ("idle", None, "Ready"),
            Self::Thinking => ("working", None, "Thinking"),
            Self::Responding => ("working", None, "Responding"),
            Self::Tool => ("working", None, "Executing a tool"),
            Self::Compacting => ("working", None, "Compacting context"),
            Self::Permission => ("blocked", Some("permission"), "Tool approval required"),
            Self::Auth => ("blocked", Some("auth"), "Complete subscription login"),
            Self::Done => ("done", None, "Finished"),
            Self::Error => ("error", None, "Stopped with an error"),
        };
        let kind = kind.map(|kind| format!(":kind={kind}")).unwrap_or_default();
        // Use our own child record so we never clear/overwrite another program's root status.
        format!(
            "\x1b]7501;state={state}:id=habibi:app=habibi{kind}:msg={}\x1b\\",
            STANDARD.encode(message)
        )
    }
}

pub(crate) struct Reporter<W: Write> {
    writer: W,
    enabled: bool,
    last: Option<Status>,
}

impl Reporter<io::Stdout> {
    pub fn terminal() -> Self {
        Self::new(
            io::stdout(),
            io::stdout().is_terminal()
                && std::env::var_os("HABIBI_NO_PROGRAM_STATUS").is_none_or(|v| v != "1"),
        )
    }
}

impl<W: Write> Reporter<W> {
    fn new(writer: W, enabled: bool) -> Self {
        Self {
            writer,
            enabled,
            last: None,
        }
    }

    pub fn report(&mut self, status: Status) {
        if !self.enabled || self.last == Some(status) {
            return;
        }
        // Best-effort terminal metadata must never abort an agent run.
        if self
            .writer
            .write_all(status.sequence().as_bytes())
            .and_then(|()| self.writer.flush())
            .is_err()
        {
            self.enabled = false;
            return;
        }
        self.last = Some(status);
    }

    pub fn acknowledge(&mut self) {
        if matches!(self.last, Some(Status::Done | Status::Error)) {
            self.report(Status::Idle);
        }
    }

    pub fn event(&mut self, event: &Event, yolo: bool) {
        let status = match event {
            Event::RunStarted | Event::Compacted { .. } | Event::ToolFinished { .. } => {
                Status::Thinking
            }
            Event::CompactionStarted => Status::Compacting,
            Event::TextDelta(_) => Status::Responding,
            Event::Permission { .. } if !yolo => Status::Permission,
            Event::ToolStarted(_) => Status::Tool,
            Event::RunFinished {
                cancelled: true, ..
            } => Status::Idle,
            Event::RunFinished { error: Some(_), .. } => Status::Error,
            Event::RunFinished { .. } => Status::Done,
            _ => return,
        };
        self.report(status);
    }
}

impl<W: Write> Drop for Reporter<W> {
    fn drop(&mut self) {
        // Never leave working/blocked stuck after cancellation or unwinding.
        // Completed/error records deliberately survive normal exit, per the protocol.
        if matches!(
            self.last,
            Some(
                Status::Thinking
                    | Status::Responding
                    | Status::Tool
                    | Status::Compacting
                    | Status::Permission
                    | Status::Auth
            )
        ) {
            self.report(Status::Idle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[test]
    fn wire_format_repeats_identity_and_encodes_fixed_messages() {
        for status in [
            Status::Idle,
            Status::Thinking,
            Status::Responding,
            Status::Tool,
            Status::Compacting,
            Status::Permission,
            Status::Auth,
            Status::Done,
            Status::Error,
        ] {
            let sequence = status.sequence();
            assert!(sequence.starts_with("\x1b]7501;state="));
            assert!(sequence.ends_with("\x1b\\"));
            assert!(sequence.contains(":id=habibi:app=habibi"));
            assert!(sequence.len() < 4096);
            let message = sequence
                .split(":msg=")
                .nth(1)
                .unwrap()
                .trim_end_matches("\x1b\\");
            let decoded = String::from_utf8(STANDARD.decode(message).unwrap()).unwrap();
            assert!(!decoded.chars().any(char::is_control));
        }
        assert!(
            Status::Permission
                .sequence()
                .contains("state=blocked:id=habibi:app=habibi:kind=permission")
        );
        assert!(Status::Auth.sequence().contains(":kind=auth:"));
    }

    #[test]
    fn transitions_deduplicate_do_not_expose_data_and_skip_yolo_permissions() {
        let mut reporter = Reporter::new(Vec::new(), true);
        reporter.event(&Event::RunStarted, false);
        reporter.event(&Event::TextDelta("SECRET prompt".into()), false);
        let length = reporter.writer.len();
        reporter.event(&Event::TextDelta("more private text".into()), false);
        assert_eq!(reporter.writer.len(), length);
        let (reply, _rx) = oneshot::channel();
        let event = Event::Permission {
            call: libhabibi::ToolCall {
                id: "private".into(),
                name: "private".into(),
                arguments: serde_json::json!({"secret":"private"}),
            },
            reply,
        };
        reporter.event(&event, true);
        assert_eq!(reporter.last, Some(Status::Responding));
        reporter.event(&event, false);
        assert_eq!(reporter.last, Some(Status::Permission));
        reporter.event(
            &Event::RunFinished {
                cancelled: true,
                error: Some("private".into()),
            },
            false,
        );
        assert_eq!(reporter.last, Some(Status::Idle));
        reporter.event(
            &Event::RunFinished {
                cancelled: false,
                error: Some("private".into()),
            },
            false,
        );
        assert_eq!(reporter.last, Some(Status::Error));
        reporter.acknowledge();
        assert_eq!(reporter.last, Some(Status::Idle));
        assert!(
            !String::from_utf8(reporter.writer.clone())
                .unwrap()
                .contains("private")
        );
    }

    #[test]
    fn active_status_is_reset_on_drop_but_completed_records_survive() {
        for status in [
            Status::Thinking,
            Status::Permission,
            Status::Auth,
            Status::Done,
            Status::Error,
        ] {
            let mut bytes = Vec::new();
            {
                let mut reporter = Reporter::new(&mut bytes, true);
                reporter.report(status);
            }
            let output = String::from_utf8(bytes).unwrap();
            if matches!(status, Status::Done | Status::Error) {
                assert_eq!(output, status.sequence());
            } else {
                assert!(output.ends_with(&Status::Idle.sequence()));
            }
        }
    }

    #[test]
    fn disabled_reporter_writes_nothing() {
        let mut reporter = Reporter::new(Vec::new(), false);
        reporter.report(Status::Thinking);
        assert!(reporter.writer.is_empty());
    }
}
