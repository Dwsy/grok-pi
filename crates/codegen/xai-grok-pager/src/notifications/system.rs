//! Best-effort native desktop notifications for user-input requests.
//!
//! The pager owns focus, so it is the only layer that can decide whether an
//! interaction needs an out-of-window notification. Delivery failures are
//! intentionally non-fatal: the in-terminal QuestionView remains authoritative.

#[cfg(not(test))]
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    MacOs,
    Linux,
    Windows,
    Unsupported,
}

#[derive(Debug, PartialEq, Eq)]
struct CommandSpec {
    program: &'static str,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

const POWERSHELL_TOAST: &str = r#"
$type = 'Windows.UI.Notifications'
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
$xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$text = $xml.GetElementsByTagName('text')
$text[0].AppendChild($xml.CreateTextNode($env:GROK_NOTIFICATION_TITLE)) > $null
$text[1].AppendChild($xml.CreateTextNode($env:GROK_NOTIFICATION_BODY)) > $null
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('grok-pi').Show($toast)
"#;

fn current_platform() -> Platform {
    #[cfg(target_os = "macos")]
    {
        return Platform::MacOs;
    }
    #[cfg(target_os = "linux")]
    {
        return Platform::Linux;
    }
    #[cfg(target_os = "windows")]
    {
        return Platform::Windows;
    }
    #[allow(unreachable_code)]
    Platform::Unsupported
}

fn command_spec(platform: Platform, title: &str, body: &str) -> Option<CommandSpec> {
    match platform {
        Platform::MacOs => Some(CommandSpec {
            program: "osascript",
            args: [
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                title,
                body,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            env: Vec::new(),
        }),
        Platform::Linux => Some(CommandSpec {
            program: "notify-send",
            args: vec!["--app-name=grok-pi".into(), title.into(), body.into()],
            env: Vec::new(),
        }),
        Platform::Windows => Some(CommandSpec {
            program: "powershell.exe",
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                POWERSHELL_TOAST.into(),
            ],
            env: vec![
                ("GROK_NOTIFICATION_TITLE".into(), title.into()),
                ("GROK_NOTIFICATION_BODY".into(), body.into()),
            ],
        }),
        Platform::Unsupported => None,
    }
}

#[cfg(not(test))]
pub(crate) fn notify(title: &str, body: &str) {
    let Some(spec) = command_spec(current_platform(), title, body) else {
        return;
    };

    let spawn = std::thread::Builder::new()
        .name("grok-system-notification".into())
        .spawn(move || {
            let mut command = Command::new(spec.program);
            command
                .args(spec.args)
                .envs(spec.env)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            xai_tty_utils::detach_std_command(&mut command);
            if let Err(error) = command.status() {
                tracing::debug!(%error, "native system notification unavailable");
            }
        });
    if let Err(error) = spawn {
        tracing::debug!(%error, "failed to start native system notification worker");
    }
}

#[cfg(test)]
thread_local! {
    static RECORDED: std::cell::RefCell<Vec<(String, String)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

#[cfg(test)]
pub(crate) fn notify(title: &str, body: &str) {
    RECORDED.with(|recorded| recorded.borrow_mut().push((title.into(), body.into())));
}

#[cfg(test)]
pub(crate) fn take_recorded_notifications() -> Vec<(String, String)> {
    RECORDED.with(|recorded| std::mem::take(&mut *recorded.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_passes_content_as_arguments() {
        let spec = command_spec(Platform::MacOs, "title", "body").unwrap();
        assert_eq!(spec.program, "osascript");
        assert_eq!(&spec.args[spec.args.len() - 2..], ["title", "body"]);
        assert!(spec.env.is_empty());
    }

    #[test]
    fn linux_uses_notify_send() {
        let spec = command_spec(Platform::Linux, "title", "body").unwrap();
        assert_eq!(spec.program, "notify-send");
        assert_eq!(spec.args, ["--app-name=grok-pi", "title", "body"]);
    }

    #[test]
    fn windows_keeps_content_out_of_the_script() {
        let spec = command_spec(Platform::Windows, "unsafe ' title", "unsafe $ body").unwrap();
        assert_eq!(spec.program, "powershell.exe");
        assert!(!spec.args.last().unwrap().contains("unsafe"));
        assert_eq!(
            spec.env,
            [
                ("GROK_NOTIFICATION_TITLE".into(), "unsafe ' title".into()),
                ("GROK_NOTIFICATION_BODY".into(), "unsafe $ body".into()),
            ]
        );
    }

    #[test]
    fn unsupported_platform_is_a_noop() {
        assert!(command_spec(Platform::Unsupported, "title", "body").is_none());
    }
}
