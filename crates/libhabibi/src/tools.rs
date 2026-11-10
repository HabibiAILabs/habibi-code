use crate::ToolCall;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_FILE: usize = 2 * 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;

pub(crate) struct Definition {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

pub(crate) fn definitions() -> Vec<Definition> {
    vec![
        Definition {
            name: "read",
            description: "Read a UTF-8 workspace file with line numbers. Maximum file size 2 MiB.",
            schema: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":5000}},"required":["path"],"additionalProperties":false}),
        },
        Definition {
            name: "write",
            description: "Atomically create or replace a UTF-8 workspace file. Parent directory must exist. Maximum 2 MiB.",
            schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
        },
        Definition {
            name: "edit",
            description: "Replace exactly one occurrence of old_text in a workspace file. Fails on missing or ambiguous matches.",
            schema: json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"],"additionalProperties":false}),
        },
        Definition {
            name: "bash",
            description: "Run a shell command in the workspace. Not sandboxed. Timeout 120 seconds; output bounded to 64 KiB per stream.",
            schema: json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}),
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    #[serde(default = "one")]
    offset: usize,
    #[serde(default = "two_hundred")]
    limit: usize,
}
fn one() -> usize {
    1
}
fn two_hundred() -> usize {
    200
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    old_text: String,
    new_text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    command: String,
}

pub(crate) async fn execute(root: &Path, call: &ToolCall) -> Result<String> {
    if call.name == "bash" {
        let args: BashArgs =
            serde_json::from_value(call.arguments.clone()).context("Invalid bash arguments")?;
        ensure!(
            !args.command.is_empty() && args.command.len() <= MAX_OUTPUT,
            "Shell command is empty or too large"
        );
        return shell(root, &args.command).await;
    }
    let root = root.to_owned();
    let call = call.clone();
    tokio::task::spawn_blocking(move || execute_file(&root, &call)).await?
}

fn execute_file(root: &Path, call: &ToolCall) -> Result<String> {
    match call.name.as_str() {
        "read" => {
            let args: ReadArgs =
                serde_json::from_value(call.arguments.clone()).context("Invalid read arguments")?;
            ensure!(
                args.offset >= 1 && (1..=5000).contains(&args.limit),
                "Read offset must be >=1 and limit 1..5000"
            );
            let contents = read_file(&resolve(root, &args.path, false)?)?;
            let mut output = String::new();
            for (index, line) in contents
                .lines()
                .enumerate()
                .skip(args.offset - 1)
                .take(args.limit)
            {
                output.push_str(&format!("{}: {line}\n", index + 1));
                if output.len() > MAX_OUTPUT {
                    break;
                }
            }
            Ok(bound_output(output))
        }
        "write" => {
            let args: WriteArgs = serde_json::from_value(call.arguments.clone())
                .context("Invalid write arguments")?;
            let path = resolve(root, &args.path, true)?;
            atomic_write(&path, &args.content)?;
            Ok(format!(
                "Wrote {} bytes to {}",
                args.content.len(),
                path.display()
            ))
        }
        "edit" => {
            let args: EditArgs =
                serde_json::from_value(call.arguments.clone()).context("Invalid edit arguments")?;
            ensure!(!args.old_text.is_empty(), "old_text cannot be empty");
            let path = resolve(root, &args.path, false)?;
            let contents = read_file(&path)?;
            let updated = replace_once(&contents, &args.old_text, &args.new_text)?;
            atomic_write(&path, &updated)?;
            Ok(format!("Edited {}", path.display()))
        }
        _ => bail!("Unknown tool: {}", call.name),
    }
}

fn resolve(root: &Path, requested: &str, allow_new: bool) -> Result<PathBuf> {
    let candidate = root.join(requested);
    let path = match fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(e) if allow_new && e.kind() == std::io::ErrorKind::NotFound => {
            let parent = candidate
                .parent()
                .context("Missing parent directory")?
                .canonicalize()?;
            parent.join(candidate.file_name().context("Missing filename")?)
        }
        Err(e) => return Err(e.into()),
    };
    ensure!(
        path.starts_with(root),
        "File tools cannot access paths outside the workspace"
    );
    ensure!(path != root, "Expected a file, not workspace root");
    // Reject dangling symlinks as new destinations rather than replacing a link unexpectedly.
    if allow_new && fs::symlink_metadata(&candidate).is_ok() && !candidate.exists() {
        bail!("Dangling symlink destination");
    }
    Ok(path)
}

fn read_file(path: &Path) -> Result<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure!(file.metadata()?.is_file(), "Expected a regular file");
    let mut bytes = Vec::new();
    file.take((MAX_FILE + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_FILE,
        "File exceeds 2 MiB; use a targeted shell command"
    );
    String::from_utf8(bytes).context("File is not valid UTF-8")
}

fn replace_once(contents: &str, old: &str, new: &str) -> Result<String> {
    ensure!(!old.is_empty(), "old_text cannot be empty");
    let start = contents
        .find(old)
        .context("old_text not found; read the file again")?;
    // Count overlapping matches too, not just str::matches' non-overlapping matches.
    let next_char = contents[start..].chars().next().unwrap().len_utf8();
    ensure!(
        !contents[start + next_char..].contains(old),
        "old_text is ambiguous; provide more context"
    );
    let mut updated = contents.to_owned();
    updated.replace_range(start..start + old.len(), new);
    Ok(updated)
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    ensure!(contents.len() <= MAX_FILE, "Write exceeds 2 MiB");
    let temporary = path.with_file_name(format!(".habibi-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = crate::auth::private_open(&temporary, true)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn bound_output(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[output truncated]");
    }
    text
}

async fn drain(mut stream: impl AsyncRead + Unpin) -> Result<String> {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let keep = count.min(MAX_OUTPUT.saturating_sub(kept.len()));
        kept.extend_from_slice(&buffer[..keep]);
        truncated |= keep != count;
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if truncated {
        text.push_str("\n[output truncated]");
    }
    Ok(text)
}

#[cfg(unix)]
struct ProcessGroup(u32);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // The child is assigned its own process group before exec. Kill descendants on
        // cancellation/timeout too, not just the shell. Shell commands are NOT sandboxed.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

async fn shell(root: &Path, command: &str) -> Result<String> {
    let mut builder = tokio::process::Command::new("bash");
    builder
        .args(["-c", command])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    builder.process_group(0);
    let mut child = builder.spawn().context("Failed to start bash")?;
    #[cfg(unix)]
    let _group = ProcessGroup(child.id().context("Missing subprocess ID")?);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let operation = async {
        let (status, stdout, stderr) = tokio::join!(child.wait(), drain(stdout), drain(stderr));
        let status = status?;
        let output = format!("exit: {status}\nstdout:\n{}\nstderr:\n{}", stdout?, stderr?);
        ensure!(status.success(), "{output}");
        Ok::<_, anyhow::Error>(output)
    };
    tokio::time::timeout(Duration::from_secs(120), operation)
        .await
        .context("Shell timed out after 120 seconds")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_requires_one_exact_match() {
        assert_eq!(replace_once("a b c", "b", "B").unwrap(), "a B c");
        assert!(replace_once("a a", "a", "b").is_err());
        assert!(replace_once("aaa", "aa", "b").is_err());
        assert!(replace_once("a", "x", "b").is_err());
        assert!(replace_once("a", "", "b").is_err());
    }
    #[test]
    fn rejects_workspace_escape_and_preserves_original_on_failed_edit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert!(resolve(&root, "../outside", true).is_err());
        fs::write(root.join("file"), "a a").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "edit".into(),
            arguments: json!({"path":"file","old_text":"a","new_text":"b"}),
        };
        assert!(execute_file(&root, &call).is_err());
        assert_eq!(fs::read_to_string(root.join("file")).unwrap(), "a a");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("escape")).unwrap();
            assert!(resolve(&root, "escape", false).is_err());
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_shell_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let mut operation = Box::pin(shell(
            temp.path(),
            "printf ready > ready; (sleep 0.3; printf bad > survived) & wait",
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                tokio::select! {
                    result = &mut operation => panic!("shell finished before cancellation: {result:?}"),
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {
                        if temp.path().join("ready").exists() { break; }
                    }
                }
            }
        }).await.unwrap();
        drop(operation);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!temp.path().join("survived").exists());
    }

    #[tokio::test]
    async fn shell_drains_bounded_output_and_reports_exit() {
        let temp = tempfile::tempdir().unwrap();
        let output = shell(temp.path(), "printf hello; printf error >&2; exit 7")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            output.contains("exit status: 7")
                && output.contains("hello")
                && output.contains("error")
        );
        let output = shell(temp.path(), "head -c 100000 /dev/zero")
            .await
            .unwrap();
        assert!(output.contains("[output truncated]") && output.len() < MAX_OUTPUT + 200);
    }
}
