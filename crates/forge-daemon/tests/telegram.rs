//! The bot against a Telegram that is not Telegram.
//!
//! A stub server stands in for the Bot API, so the long poll, the allowlist and
//! the command replies can be driven end to end without a bot token or a phone.

mod harness;

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use forge_daemon::config::TelegramConfig;
use forge_daemon::fleet::Fleet;
use forge_daemon::telegram::api::{BotToken, Telegram};
use forge_daemon::telegram::bot::Bot;
use harness::Harness;
use serde_json::{json, Value};

/// What the stub has been asked, and what it will answer with.
#[derive(Default)]
struct Exchange {
    /// Updates handed out on the next `getUpdates`, then never again.
    pending: Vec<Value>,
    /// Every `sendMessage` body the bot sent.
    sent: Vec<Value>,
    /// Every `answerCallbackQuery` text.
    acknowledged: Vec<String>,
}

type Shared = Arc<Mutex<Exchange>>;

async fn method(
    State(shared): State<Shared>,
    Path((_, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut exchange = shared.lock().unwrap();

    match method.as_str() {
        "getUpdates" => {
            let updates = std::mem::take(&mut exchange.pending);
            Json(json!({ "ok": true, "result": updates }))
        }
        "sendMessage" => {
            // Telegram rejects a MarkdownV2 message with an unescaped reserved
            // character, and only logs on the bot's side. The stub does the
            // same, so an escaping regression fails a test instead of hiding.
            if let Some(bad) = unescaped_markdown(body["text"].as_str().unwrap_or_default()) {
                return Json(json!({
                    "ok": false,
                    "description": format!("Bad Request: character '{bad}' is reserved"),
                }));
            }

            exchange.sent.push(body);
            Json(json!({ "ok": true, "result": { "message_id": 1, "chat": { "id": 1 } } }))
        }
        "answerCallbackQuery" => {
            exchange
                .acknowledged
                .push(body["text"].as_str().unwrap_or_default().to_owned());
            Json(json!({ "ok": true, "result": true }))
        }
        _ => Json(json!({ "ok": true, "result": true })),
    }
}

/// The first reserved character sitting unescaped outside a code fence.
fn unescaped_markdown(text: &str) -> Option<char> {
    const RESERVED: [char; 18] = [
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
    ];

    // Fenced blocks have their own escaping rules; only prose is checked here.
    let prose: Vec<&str> = text.split("```").step_by(2).collect();

    for part in prose {
        let mut chars = part.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                chars.next();
                continue;
            }
            if RESERVED.contains(&c) {
                return Some(c);
            }
        }
    }

    None
}

/// A stub Bot API on a loopback port, and the record of what passed through it.
async fn stub_telegram() -> (String, Shared) {
    let shared: Shared = Arc::default();

    let app = Router::new()
        .route("/{token}/{method}", post(method))
        .with_state(Arc::clone(&shared));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{addr}"), shared)
}

/// A message update, as Telegram would deliver one.
fn message(update_id: i64, from: i64, text: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "chat": { "id": from },
            "from": { "id": from },
            "text": text,
        },
    })
}

/// Run the bot until it has sent `expected` messages, or time out.
async fn run_until(bot: Bot, shared: &Shared, expected: usize) -> Vec<Value> {
    let running = tokio::spawn(bot.run());

    for _ in 0..200 {
        {
            let exchange = shared.lock().unwrap();
            if exchange.sent.len() >= expected {
                let sent = exchange.sent.clone();
                running.abort();
                return sent;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    running.abort();
    let sent = shared.lock().unwrap().sent.clone();
    panic!("the bot sent {} messages, expected {expected}", sent.len());
}

fn bot_for(harness: &Harness, base: &str, allowed: Vec<i64>) -> Bot {
    let config = TelegramConfig {
        enabled: true,
        allowed_user_ids: allowed,
        token_ref: "unused".to_owned(),
    };

    let telegram = Telegram::with_base(BotToken::new("stub-token"), base).unwrap();
    let fleet = Fleet::new(harness.state.clone(), &[], &[]);

    Bot::new(telegram, fleet, harness.state.clone(), &config, Vec::new())
}

#[tokio::test]
async fn an_allowlisted_sender_gets_an_answer() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![message(1, 7, "/status")];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["chat_id"], 7);
    // No projects registered, so this Mac has nothing running.
    assert!(
        sent[0]["text"]
            .as_str()
            .unwrap()
            .contains("nothing running"),
        "{}",
        sent[0]["text"]
    );
}

#[tokio::test]
async fn a_sender_not_on_the_allowlist_gets_silence() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![
        message(1, 999, "/status"),
        // An allowlisted message after it, so the test can tell "dropped" from
        // "the bot had not got there yet".
        message(2, 7, "/help"),
    ];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    assert_eq!(sent.len(), 1, "only the allowlisted sender is answered");
    assert_eq!(sent[0]["chat_id"], 7);
}

#[tokio::test]
async fn help_lists_every_command_the_spec_promises() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![message(1, 7, "/help")];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;
    let text = sent[0]["text"].as_str().unwrap();

    for command in ["/status", "/agents", "/start", "/stop", "/ask"] {
        assert!(text.contains(command), "{command} missing from {text}");
    }
}

#[tokio::test]
async fn a_command_the_bot_does_not_know_says_what_it_does_know() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![message(1, 7, "/deploy everything")];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    assert!(sent[0]["text"]
        .as_str()
        .unwrap()
        .contains("I only understand"));
}

#[tokio::test]
async fn chatter_that_is_not_a_command_gets_no_reply() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![message(1, 7, "morning all"), message(2, 7, "/agents")];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    assert_eq!(
        sent.len(),
        1,
        "the bot answered the command and nothing else"
    );
    assert!(sent[0]["text"]
        .as_str()
        .unwrap()
        .contains("Nothing is running"));
}

#[tokio::test]
async fn stopping_something_that_does_not_exist_says_so() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![message(1, 7, "/stop nonesuch")];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    assert!(sent[0]["text"]
        .as_str()
        .unwrap()
        .contains("No task matches"));
}

#[tokio::test]
async fn the_offset_is_remembered_so_a_restart_does_not_answer_twice() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;
    shared.lock().unwrap().pending = vec![message(41, 7, "/status")];

    run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    // Telegram treats the offset as confirmation of everything before it.
    let stored = harness
        .state
        .store
        .setting("telegram.offset")
        .unwrap()
        .expect("the offset is written down");

    assert_eq!(stored, "42");
}

#[tokio::test]
async fn a_tap_on_a_question_nobody_asked_answers_nothing() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;

    shared.lock().unwrap().pending = vec![json!({
        "update_id": 1,
        "callback_query": {
            "id": "tap-1",
            "from": { "id": 7 },
            // A well-formed payload for a session with no outstanding prompt.
            "data": "y:l:12:3:941",
        },
    })];

    let bot = bot_for(&harness, &base, vec![7]);
    let running = tokio::spawn(bot.run());

    let mut acknowledged = Vec::new();
    for _ in 0..200 {
        {
            let exchange = shared.lock().unwrap();
            if !exchange.acknowledged.is_empty() {
                acknowledged = exchange.acknowledged.clone();
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    running.abort();

    assert_eq!(acknowledged.len(), 1);
    assert!(
        acknowledged[0].contains("answered already"),
        "{}",
        acknowledged[0]
    );
}

#[tokio::test]
async fn a_tap_from_someone_not_on_the_allowlist_is_not_even_acknowledged() {
    let harness = Harness::new();
    let (base, shared) = stub_telegram().await;

    shared.lock().unwrap().pending = vec![
        json!({
            "update_id": 1,
            "callback_query": {
                "id": "tap-1",
                "from": { "id": 999 },
                "data": "y:l:12:3:941",
            },
        }),
        message(2, 7, "/help"),
    ];

    let sent = run_until(bot_for(&harness, &base, vec![7]), &shared, 1).await;

    assert_eq!(sent.len(), 1);
    assert!(
        shared.lock().unwrap().acknowledged.is_empty(),
        "a stranger's tap is dropped, not answered"
    );
}
