//! Restricted JavaScript orchestration. The VM has no OS/network/module APIs.
//! Tool calls are bridged back to the session's normal permission dispatcher.
use crate::{CancellationToken, ToolDefinition, ToolOutput};
use anyhow::{Result, ensure};
use rquickjs::{Context, Function, Promise, Runtime};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, LazyLock, mpsc},
    time::{Duration, Instant},
};

pub(crate) static WORKERS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
pub(crate) const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: usize = 64 * 1024;

pub(crate) struct Request {
    pub name: String,
    pub arguments: Value,
    pub reply: mpsc::Sender<std::result::Result<ToolOutput, String>>,
}

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "codemode".into(),
        description: "Run an async JavaScript function body in a restricted VM. Use await tools[name](args), text(value), console.log(value), return value, searchTools(query), describeTool(name). No filesystem, network, Node APIs, imports or timers. Every nested tool call requires separate approval. Use Promise.all or Promise.allSettled for parallel calls (up to 8 in flight). Approval prompts are serialized. Await dependent operations explicitly. Built-ins return text; MCP tools return their full CallToolResult. Only explicit output reaches the model; child calls are audited. No nested codemode. Limits: 120 seconds including approval waits, 64 MiB memory, 64 KiB output, 64 calls. Completed side effects are not rolled back on failure.".into(),
        schema: json!({"type":"object","properties":{"code":{"type":"string","minLength":1,"maxLength":65536}},"required":["code"],"additionalProperties":false}),
    }
}

pub(crate) fn run(
    code: String,
    definitions: Vec<ToolDefinition>,
    requests: tokio::sync::mpsc::Sender<Request>,
    cancel: CancellationToken,
    deadline: Instant,
) -> Result<ToolOutput> {
    ensure!(code.len() <= 65536, "Script exceeds 64 KiB");
    let runtime = Runtime::new()?;
    runtime.set_memory_limit(64 * 1024 * 1024);
    runtime.set_max_stack_size(1024 * 1024);
    let interrupt = cancel.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        interrupt.is_cancelled() || Instant::now() >= deadline
    })));
    let context = Context::full(&runtime)?;
    let output = Rc::new(RefCell::new(String::new()));
    let captured = output.clone();
    fn evaluate<'js>(
        ctx: rquickjs::Ctx<'js>,
        code: String,
        definitions: Vec<ToolDefinition>,
        requests: tokio::sync::mpsc::Sender<Request>,
        cancel: CancellationToken,
        deadline: Instant,
        captured: Rc<RefCell<String>>,
    ) -> Result<()> {
        let calls = RefCell::new(0usize);
        let pending = Rc::new(RefCell::new(Vec::new()));
        let pending_calls = pending.clone();
        let bridge_cancel = cancel.clone();
        ctx.globals().set(
            "__call",
            Function::new(
                ctx.clone(),
                move |bridge_ctx: rquickjs::Ctx<'js>, name: String, args: String| {
                    let (promise, resolve, _reject) = Promise::new(&bridge_ctx)?;
                    let enqueue =
                        || -> Result<mpsc::Receiver<std::result::Result<ToolOutput, String>>> {
                            ensure!(
                                !bridge_cancel.is_cancelled() && Instant::now() < deadline,
                                "Script cancelled or timed out"
                            );
                            ensure!(name != "codemode", "Nested codemode is not allowed");
                            ensure!(args.len() <= 1024 * 1024, "Tool arguments exceed 1 MiB");
                            *calls.borrow_mut() += 1;
                            ensure!(*calls.borrow() <= 64, "Script exceeds 64 tool calls");
                            let (reply, response) = mpsc::channel();
                            requests
                                .try_send(Request {
                                    name,
                                    arguments: serde_json::from_str(&args)?,
                                    reply,
                                })
                                .map_err(|_| {
                                    anyhow::anyhow!("Session disconnected or tool queue full")
                                })?;
                            Ok(response)
                        };
                    match enqueue() {
                        Ok(response) => pending_calls.borrow_mut().push((response, resolve)),
                        Err(error) => resolve.call::<_, ()>((
                            json!({"ok":false,"error":format!("{error:#}")}).to_string(),
                        ))?,
                    }
                    Ok::<_, rquickjs::Error>(promise)
                },
            )?,
        )?;
        ctx.globals().set(
            "__text",
            Function::new(ctx.clone(), move |text: String| -> bool {
                let mut output = captured.borrow_mut();
                if output.len() + text.len() + 1 > MAX_OUTPUT {
                    return false;
                }
                output.push_str(&text);
                output.push('\n');
                true
            })?,
        )?;
        let catalog = serde_json::to_string(
            &definitions
                .into_iter()
                .filter(|d| d.name != "codemode")
                .collect::<Vec<_>>(),
        )?;
        // Capture bridge and catalog in a closure; no generated tool names are
        // interpolated as executable source.
        let setup = format!(
            r#"
            (() => {{
                const call = globalThis.__call, emit = globalThis.__text;
                delete globalThis.__call; delete globalThis.__text;
                const catalog = {catalog};
                globalThis.tools = Object.create(null);
                for (const tool of catalog) tools[tool.name] = async (args) => {{
                    const result = JSON.parse(await call(tool.name, JSON.stringify(args)));
                    if (!result.ok) throw new Error(result.error);
                    return result.value;
                }};
                globalThis.text = (value) => {{
                    const rendered = typeof value === 'string' ? value : JSON.stringify(value);
                    if (!emit(rendered === undefined ? 'undefined' : rendered)) throw new Error('Script output exceeds 64 KiB');
                }};
                globalThis.console = {{ log: (...args) => text(args), info: (...args) => text(args), warn: (...args) => text(args), error: (...args) => text(args) }};
                globalThis.describeTool = name => catalog.find(t => t.name === name);
                globalThis.searchTools = query => {{
                    const words = String(query).toLowerCase().split(/\s+/).filter(Boolean);
                    return catalog.filter(t => words.every(w => (t.name + ' ' + t.description).toLowerCase().includes(w))).slice(0, 20);
                }};
            }})()
        "#
        );
        ctx.eval::<(), _>(setup)?;
        let script = format!(
            "(async () => {{\n{code}\n}})().then(value => {{ if (value !== undefined) text(value); }})"
        );
        let result = ctx.eval::<Promise, _>(script).and_then(|promise| {
            loop {
                if cancel.is_cancelled() || Instant::now() >= deadline {
                    return Err(rquickjs::Error::WouldBlock);
                }
                if let Some(result) = promise.result::<()>() { return result; }
                // Pump JS continuations, then settle every ready host-call promise.
                // No host callback blocks the VM, so Promise.all can enqueue all
                // independent work before any result is available.
                let mut progressed = ctx.execute_pending_job();
                let mut ready = Vec::new();
                {
                    let mut pending = pending.borrow_mut();
                    let mut index = 0;
                    while index < pending.len() {
                        let result = match pending[index].0.try_recv() {
                            Ok(result) => Some(result),
                            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Session disconnected".into())),
                            Err(mpsc::TryRecvError::Empty) => None,
                        };
                        if let Some(result) = result {
                            let (_, resolve) = pending.swap_remove(index);
                            ready.push((resolve, result));
                        } else { index += 1; }
                    }
                }
                for (resolve, result) in ready {
                    let envelope = match result {
                        Ok(result) if !result.is_error => json!({"ok":true,"value":result.structured.unwrap_or(Value::String(result.text))}),
                        Ok(result) => json!({"ok":false,"error":result.render()}),
                        Err(error) => json!({"ok":false,"error":error}),
                    };
                    resolve.call::<_, ()>((envelope.to_string(),))?;
                    progressed = true;
                }
                if !progressed {
                    if pending.borrow().is_empty() { return Err(rquickjs::Error::WouldBlock); }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });
        pending.borrow_mut().clear();
        result.map_err(|error| {
            if error.is_exception() {
                let exception = ctx.catch();
                anyhow::anyhow!("JavaScript exception: {:?}", exception)
            } else {
                anyhow::anyhow!("JavaScript execution failed: {error}")
            }
        })
    }
    let result =
        context.with(|ctx| evaluate(ctx, code, definitions, requests, cancel, deadline, captured));
    let mut text = output.borrow().clone();
    let is_error = result.is_err();
    if let Err(error) = result {
        let mut error = format!("Script failed: {error:#}\n");
        truncate(&mut error, 2048);
        truncate(&mut text, MAX_OUTPUT - error.len());
        text.push_str(&error);
    }
    Ok(ToolOutput {
        text,
        structured: None,
        is_error,
    })
}

fn truncate(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evaluate(code: &str, timeout: Duration) -> ToolOutput {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        run(
            code.into(),
            vec![],
            tx,
            CancellationToken::new(),
            Instant::now() + timeout,
        )
        .unwrap()
    }
    #[test]
    fn async_output_and_no_host_apis() {
        let result = evaluate(
            "text(await Promise.resolve(42)); return [typeof process, typeof fetch, typeof require, typeof setTimeout];",
            Duration::from_secs(2),
        );
        assert!(!result.is_error, "{}", result.text);
        assert!(result.text.contains("42"));
        assert!(
            result
                .text
                .contains("[\"undefined\",\"undefined\",\"undefined\",\"undefined\"]")
        );
    }
    #[test]
    fn unawaited_calls_release_vm_resources() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let result = run(
            "tools.example({}); return 'finished';".into(),
            vec![ToolDefinition {
                name: "example".into(),
                description: "test".into(),
                schema: json!({"type":"object"}),
            }],
            tx,
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(!result.is_error, "{}", result.text);
        let request = rx.try_recv().unwrap();
        assert!(request.reply.send(Ok(ToolOutput::text("late"))).is_err());
    }

    #[test]
    fn infinite_loop_and_unresolved_promise_fail() {
        assert!(evaluate("while (true) {}", Duration::from_millis(30)).is_error);
        assert!(evaluate("await new Promise(() => {});", Duration::from_secs(1)).is_error);
    }
    #[test]
    fn output_limit_and_partial_failure() {
        let result = evaluate(
            "text('before'); text('x'.repeat(70000));",
            Duration::from_secs(1),
        );
        assert!(result.is_error);
        assert!(result.text.starts_with("before\n"));
        assert!(result.text.len() < 1000);
    }
    #[test]
    fn memory_and_error_output_are_bounded() {
        let result = evaluate(
            "return new Array(100000000).fill('x');",
            Duration::from_secs(2),
        );
        assert!(result.is_error);
        let result = evaluate(
            "text('x'.repeat(65000)); throw new Error('ü'.repeat(10000));",
            Duration::from_secs(2),
        );
        assert!(result.is_error);
        assert!(result.text.len() <= MAX_OUTPUT);
    }
}
