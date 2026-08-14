//! The slice of Telegram's Bot API the bot uses.
//!
//! Outbound only, and long-polling: the daemon calls out to Telegram and waits
//! there for updates. No webhook, no inbound port, nothing for anyone to find
//! (SPEC §8).

use std::time::Duration;

use serde::{Deserialize, Serialize};

const BASE: &str = "https://api.telegram.org";

/// How long Telegram holds a `getUpdates` open with nothing to say.
const LONG_POLL_SECS: u64 = 50;

/// Longer than the long poll, so a normal empty poll is never a timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(LONG_POLL_SECS + 15);

/// One update from Telegram. Only the two kinds the bot acts on are modelled;
/// the rest deserialise with both fields absent and are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    /// Needed to edit the message later — to take answered buttons off it.
    pub message_id: i64,
    pub chat: Chat,
    #[serde(default)]
    pub from: Option<User>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub message: Option<Message>,
}

/// One inline button.
#[derive(Debug, Clone, Serialize)]
pub struct Button {
    pub text: String,
    pub callback_data: String,
}

#[derive(Debug, Clone, Serialize)]
struct InlineKeyboard {
    inline_keyboard: Vec<Vec<Button>>,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

/// A bot token, kept out of every Debug line and every log.
#[derive(Clone)]
pub struct BotToken(String);

impl BotToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

impl std::fmt::Debug for BotToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BotToken(…)")
    }
}

pub struct Telegram {
    http: reqwest::Client,
    token: BotToken,
    base: String,
}

impl Telegram {
    pub fn new(token: BotToken) -> Result<Self, TelegramError> {
        Self::with_base(token, BASE)
    }

    /// Point the client somewhere else. Tests use it; nothing else should.
    pub fn with_base(token: BotToken, base: &str) -> Result<Self, TelegramError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|err| TelegramError::Transport(err.to_string()))?;

        Ok(Self {
            http,
            token,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    /// Wait for the next batch of updates after `offset`.
    ///
    /// Telegram treats asking for `offset` as confirming everything before it,
    /// which is what stops an update being handled twice across a restart.
    pub async fn get_updates(&self, offset: i64) -> Result<Vec<Update>, TelegramError> {
        self.call(
            "getUpdates",
            &serde_json::json!({
                "offset": offset,
                "timeout": LONG_POLL_SECS,
                "allowed_updates": ["message", "callback_query"],
            }),
        )
        .await
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        buttons: Vec<Vec<Button>>,
    ) -> Result<Message, TelegramError> {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "MarkdownV2",
            "disable_web_page_preview": true,
        });

        if !buttons.is_empty() {
            body["reply_markup"] = serde_json::to_value(InlineKeyboard {
                inline_keyboard: buttons,
            })
            .expect("an inline keyboard always serialises");
        }

        self.call("sendMessage", &body).await
    }

    /// Acknowledge a tap. Telegram spins the button until this arrives, so it
    /// is sent whatever the tap turned out to mean.
    pub async fn answer_callback(&self, id: &str, text: &str) -> Result<(), TelegramError> {
        let _: serde_json::Value = self
            .call(
                "answerCallbackQuery",
                &serde_json::json!({ "callback_query_id": id, "text": text }),
            )
            .await?;
        Ok(())
    }

    /// Take the buttons off a message, so an answered prompt cannot be tapped
    /// again from the scrollback.
    pub async fn clear_buttons(&self, chat_id: i64, message_id: i64) -> Result<(), TelegramError> {
        let _: serde_json::Value = self
            .call(
                "editMessageReplyMarkup",
                &serde_json::json!({ "chat_id": chat_id, "message_id": message_id }),
            )
            .await?;
        Ok(())
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, TelegramError> {
        // The token is in the path, which is how Telegram works. It is never
        // logged: the URL is built here and not kept.
        let url = format!("{}/bot{}/{method}", self.base, self.token.0);

        let response = self
            .http
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|err| TelegramError::Transport(strip_token(&err.to_string())))?;

        let envelope: Envelope<T> = response
            .json()
            .await
            .map_err(|err| TelegramError::Transport(strip_token(&err.to_string())))?;

        match (envelope.ok, envelope.result) {
            (true, Some(result)) => Ok(result),
            (true, None) => Err(TelegramError::Api(format!("{method} answered nothing"))),
            (false, _) => Err(TelegramError::Api(
                envelope
                    .description
                    .unwrap_or_else(|| format!("{method} was refused")),
            )),
        }
    }
}

/// reqwest puts the URL in its error messages, and the URL has the token in it.
fn strip_token(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;

    while let Some(at) = rest.find("/bot") {
        out.push_str(&rest[..at + 4]);
        out.push('…');
        rest = match rest[at + 4..].find('/') {
            Some(end) => &rest[at + 4 + end..],
            None => "",
        };
    }
    out.push_str(rest);

    out
}

#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    #[error("cannot reach Telegram: {0}")]
    Transport(String),
    #[error("Telegram refused: {0}")]
    Api(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_never_appears_in_debug_output() {
        let token = BotToken::new("123456:AAHwellthisissecret");

        assert_eq!(format!("{token:?}"), "BotToken(…)");
        assert!(!format!("{token:?}").contains("AAHwell"));
    }

    #[test]
    fn a_transport_error_does_not_carry_the_token_it_failed_with() {
        let raw = "error sending request for url \
                   (https://api.telegram.org/bot123456:AAHsecret/getUpdates): timed out";

        let cleaned = strip_token(raw);

        assert!(!cleaned.contains("AAHsecret"), "{cleaned}");
        assert!(cleaned.contains("getUpdates"), "{cleaned}");
    }

    #[test]
    fn stripping_leaves_a_message_with_no_token_in_it_alone() {
        assert_eq!(strip_token("connection refused"), "connection refused");
    }

    #[test]
    fn an_update_that_is_neither_a_message_nor_a_tap_still_parses() {
        // Telegram sends kinds the bot did not ask for; they must not break
        // the loop, because the offset only advances once they are consumed.
        let update: Update = serde_json::from_str(r#"{"update_id":7,"poll":{"id":"1"}}"#).unwrap();

        assert_eq!(update.update_id, 7);
        assert!(update.message.is_none());
        assert!(update.callback_query.is_none());
    }

    #[test]
    fn a_message_without_text_parses_as_one_without_text() {
        let update: Update = serde_json::from_str(
            r#"{"update_id":7,"message":{"message_id":3,"chat":{"id":5},"from":{"id":9}}}"#,
        )
        .unwrap();

        let message = update.message.expect("there is a message");
        assert_eq!(message.message_id, 3);
        assert_eq!(message.chat.id, 5);
        assert_eq!(message.from.expect("a sender").id, 9);
        assert!(message.text.is_none());
    }
}
