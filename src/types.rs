//! Question builders, request bodies, and typed API responses.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Error;

/// JSON content accepted by the API, including text, objects, arrays, and null.
pub type Entry = Value;
/// Labels mapped to descriptions; use [`Value::Null`] for an undescribed label.
pub type ChoiceCriteria = BTreeMap<String, Value>;
/// Descriptions for the `true` and `false` outcomes of a noul.
pub type NoulCriteria = BTreeMap<String, Value>;
/// An ordered rubric whose positions are the integer score levels.
pub type ScoreCriteria = Vec<Value>;
/// Questions keyed by the names used to identify their answers.
pub type Questions = BTreeMap<String, Question>;

/// A yes/no question. Its answer is a probability between zero and one.
///
/// `Noul::default()` omits instructions; `Noul::new(Value::Null)` sends explicit null.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(tag = "type", rename = "noul")]
pub struct Noul {
    /// Optional question text or structured JSON; `Some(Value::Null)` sends null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Optional descriptions of the `true` and `false` outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Value>,
}

impl Noul {
    /// Create a yes/no question with text or structured instructions.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self::default().instructions(instructions)
    }

    /// Set instructions, preserving an explicitly supplied JSON null.
    pub fn instructions(mut self, instructions: impl Into<Value>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Describe the `true` and/or `false` outcomes. Nested JSON is preserved.
    pub fn criteria<K, V>(mut self, criteria: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<Value>,
    {
        self.criteria = Some(Value::Object(json_map(criteria)));
        self
    }

    /// Send an explicit null criteria field instead of omitting it.
    pub fn null_criteria(mut self) -> Self {
        self.criteria = Some(Value::Null);
        self
    }
}

/// A question that selects among named alternatives.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename = "choice")]
pub struct Choice {
    /// Optional question text or structured JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Named alternatives and their descriptions.
    pub criteria: ChoiceCriteria,
}

impl Choice {
    /// Create a choice from label-description pairs, such as `[("yes", "Accept"), ("no", "Reject")]`.
    pub fn new<K, V>(criteria: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<Value>,
    {
        Self {
            instructions: None,
            criteria: criteria
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    /// Set instructions, preserving an explicitly supplied JSON null.
    pub fn instructions(mut self, instructions: impl Into<Value>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

/// A question that assigns an expected score using an ordered rubric.
///
/// The rubric must contain at least one entry. This follows the Python SDK;
/// the JavaScript SDK currently requires at least two entries.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename = "score")]
pub struct Score {
    /// Optional question text or structured JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Descriptions in score order, starting with level zero.
    pub criteria: ScoreCriteria,
}

impl Score {
    /// Create a score question from an ordered rubric, such as `["low", "high"]`.
    pub fn new<V: Into<Value>>(criteria: impl IntoIterator<Item = V>) -> Self {
        Self {
            instructions: None,
            criteria: criteria.into_iter().map(Into::into).collect(),
        }
    }

    /// Set instructions, preserving an explicitly supplied JSON null.
    pub fn instructions(mut self, instructions: impl Into<Value>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

/// A typed question, or a raw JSON object for additional and future API fields.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum Question {
    Noul(Noul),
    Choice(Choice),
    Score(Score),
    Raw(Value),
}

impl From<Noul> for Question {
    fn from(question: Noul) -> Self {
        Self::Noul(question)
    }
}

impl From<Choice> for Question {
    fn from(question: Choice) -> Self {
        Self::Choice(question)
    }
}

impl From<Score> for Question {
    fn from(question: Score) -> Self {
        Self::Score(question)
    }
}

impl From<Value> for Question {
    fn from(question: Value) -> Self {
        Self::Raw(question)
    }
}

/// State and named questions to evaluate with System One.
#[derive(Clone, Debug, Serialize)]
pub struct SystemOneRequest {
    /// Text or structured JSON to evaluate.
    pub state: Value,
    /// Nonempty questions keyed by answer name.
    pub questions: Questions,
    /// Optional model override; omitted values use the client's configured model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Additional JSON fields, merged after the standard fields as in the Python SDK.
    #[serde(flatten)]
    pub extra_body: Map<String, Value>,
}

impl SystemOneRequest {
    /// Create a request from state and named questions.
    ///
    /// Arrays of pairs and maps are accepted. Convert mixed question types to
    /// [`Question`] using `.into()` or `Question::from(...)`.
    pub fn new<K, Q>(state: impl Into<Value>, questions: impl IntoIterator<Item = (K, Q)>) -> Self
    where
        K: Into<String>,
        Q: Into<Question>,
    {
        Self {
            state: state.into(),
            questions: questions
                .into_iter()
                .map(|(key, question)| (key.into(), question.into()))
                .collect(),
            model: None,
            extra_body: Map::new(),
        }
    }

    /// Override the client's model for this request.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Add fields that are merged last, including explicit nulls and overrides.
    pub fn extra_body<K, V>(mut self, fields: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<Value>,
    {
        self.extra_body.extend(json_map(fields));
        self
    }

    /// Validate the questions and resolve the model before an HTTP request is sent.
    pub fn prepare(&self, default_model: &str) -> Result<Value, Error> {
        if self.questions.is_empty() {
            return Err(Error::InvalidRequest(
                "At least one question is required.".into(),
            ));
        }
        let mut questions = Map::new();
        for (name, question) in &self.questions {
            let question = serde_json::to_value(question)
                .map_err(|error| Error::InvalidRequest(error.to_string()))?;
            validate_question(name, &question)?;
            questions.insert(name.clone(), question);
        }
        let mut body = Map::new();
        body.insert("state".into(), self.state.clone());
        body.insert("questions".into(), Value::Object(questions));
        body.insert(
            "model".into(),
            Value::String(self.model.as_deref().unwrap_or(default_model).into()),
        );
        body.extend(self.extra_body.clone());
        Ok(Value::Object(body))
    }
}

fn json_map<K, V>(entries: impl IntoIterator<Item = (K, V)>) -> Map<String, Value>
where
    K: Into<String>,
    V: Into<Value>,
{
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
}

fn validate_question(name: &str, question: &Value) -> Result<(), Error> {
    let kind = question
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.is_empty());
    let Some(kind) = kind else {
        return Err(Error::InvalidRequest(format!(
            "Question {name:?} must be an object with a nonempty string \"type\"."
        )));
    };
    if matches!(kind, "choice" | "score") && question.get("criteria").is_none() {
        return Err(Error::InvalidRequest(format!(
            "Question {name:?} requires \"criteria\"."
        )));
    }
    if kind == "choice" && !question["criteria"].is_object() {
        return Err(Error::InvalidRequest(format!(
            "Choice question {name:?} requires a map of labels to descriptions."
        )));
    }
    if kind == "score" {
        let criteria = question["criteria"].as_array().ok_or_else(|| {
            Error::InvalidRequest(format!(
                "Score question {name:?} requires a list of descriptions indexed by score from zero."
            ))
        })?;
        if criteria.is_empty() {
            return Err(Error::InvalidRequest(format!(
                "Score question {name:?} has no criteria; at least one score is required."
            )));
        }
    }
    Ok(())
}

/// The complete response before typed parsing, including unrecognized API fields.
#[derive(Clone)]
pub struct RawResponse {
    /// HTTP response status.
    pub status: StatusCode,
    /// Original response headers.
    pub headers: HeaderMap,
    /// Original response bytes, retained without rewriting the JSON.
    pub body: Bytes,
}

impl RawResponse {
    /// The `x-typesafe-request-id` header, when present and valid UTF-8.
    pub fn request_id(&self) -> Option<&str> {
        self.headers
            .get("x-typesafe-request-id")
            .and_then(|value| value.to_str().ok())
    }

    /// Decode the complete response body, including unknown API fields.
    pub fn json(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }

    /// Read the body as UTF-8, replacing invalid byte sequences.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

impl fmt::Debug for RawResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawResponse")
            .field("status", &self.status)
            .field("request_id", &self.request_id())
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// A yes/no answer, expressed as the probability of yes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoulAnswer {
    pub noul: f64,
}

/// The selected label, its confidence, and the probability of each label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

/// An expected score, which may fall between integer rubric levels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreAnswer {
    pub score: f64,
    pub confidence: f64,
    /// JSON string keys are converted to integer score levels on parsing.
    pub legend: BTreeMap<i64, Value>,
    pub probabilities: BTreeMap<i64, f64>,
}

/// An answer identified by its `type` discriminator in the API response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

impl Answer {
    pub fn as_noul(&self) -> Option<&NoulAnswer> {
        match self {
            Self::Noul(answer) => Some(answer),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<&ChoiceAnswer> {
        match self {
            Self::Choice(answer) => Some(answer),
            _ => None,
        }
    }

    pub fn as_score(&self) -> Option<&ScoreAnswer> {
        match self {
            Self::Score(answer) => Some(answer),
            _ => None,
        }
    }
}

/// Token counts, when reported by the API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Answers keyed by question name, together with model and token usage metadata.
#[derive(Clone, Debug, Serialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub usage: Usage,
    pub answers: BTreeMap<String, Answer>,
    #[serde(skip)]
    pub raw_http_response: RawResponse,
}

impl SystemOneResponse {
    /// Parse an HTTP response, skipping future answer types while retaining the raw body.
    /// Non-2xx statuses produce an API error before response validation.
    pub fn from_raw(raw: RawResponse) -> Result<Self, Error> {
        Self::from_raw_with_log_level(raw, log::LevelFilter::Warn)
    }

    pub(crate) fn from_raw_with_log_level(
        raw: RawResponse,
        log_level: log::LevelFilter,
    ) -> Result<Self, Error> {
        let body = parse_response_body(&raw)?;
        let model = required(&body, "model", "model", &raw)?;
        let usage_body = body
            .get("usage")
            .filter(|value| value.is_object())
            .ok_or_else(|| invalid_response(&raw, "usage"))?;
        let usage = Usage {
            input_tokens: optional(usage_body, "input_tokens", "usage.input_tokens", &raw)?,
            output_tokens: optional(usage_body, "output_tokens", "usage.output_tokens", &raw)?,
        };
        let answer_bodies = body
            .get("answers")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_response(&raw, "answers"))?;
        let mut answers = BTreeMap::new();
        for (name, value) in answer_bodies {
            let prefix = format!("answers.{name}");
            let kind: String = required(value, "type", &format!("{prefix}.type"), &raw)?;
            let answer = match kind.as_str() {
                "noul" => Answer::Noul(NoulAnswer {
                    noul: required(value, "noul", &format!("{prefix}.noul"), &raw)?,
                }),
                "choice" => Answer::Choice(ChoiceAnswer {
                    choice: required(value, "choice", &format!("{prefix}.choice"), &raw)?,
                    confidence: required(
                        value,
                        "confidence",
                        &format!("{prefix}.confidence"),
                        &raw,
                    )?,
                    probabilities: required(
                        value,
                        "probabilities",
                        &format!("{prefix}.probabilities"),
                        &raw,
                    )?,
                }),
                "score" => Answer::Score(ScoreAnswer {
                    score: required(value, "score", &format!("{prefix}.score"), &raw)?,
                    confidence: required(
                        value,
                        "confidence",
                        &format!("{prefix}.confidence"),
                        &raw,
                    )?,
                    legend: required(value, "legend", &format!("{prefix}.legend"), &raw)?,
                    probabilities: required(
                        value,
                        "probabilities",
                        &format!("{prefix}.probabilities"),
                        &raw,
                    )?,
                }),
                _ => {
                    if log_level >= log::LevelFilter::Warn {
                        log::warn!(target: "typesafe_ai_rs", "Ignoring answer {name:?} with unrecognized type {kind:?}");
                    }
                    continue;
                }
            };
            answers.insert(name.clone(), answer);
        }
        Ok(Self {
            model,
            usage,
            answers,
            raw_http_response: raw,
        })
    }

    pub fn request_id(&self) -> Option<&str> {
        self.raw_http_response.request_id()
    }

    /// Borrow all yes/no answers without cloning their contents.
    pub fn nouls(&self) -> BTreeMap<&str, &NoulAnswer> {
        self.answers
            .iter()
            .filter_map(|(name, answer)| answer.as_noul().map(|answer| (name.as_str(), answer)))
            .collect()
    }

    pub fn choices(&self) -> BTreeMap<&str, &ChoiceAnswer> {
        self.answers
            .iter()
            .filter_map(|(name, answer)| answer.as_choice().map(|answer| (name.as_str(), answer)))
            .collect()
    }

    pub fn scores(&self) -> BTreeMap<&str, &ScoreAnswer> {
        self.answers
            .iter()
            .filter_map(|(name, answer)| answer.as_score().map(|answer| (name.as_str(), answer)))
            .collect()
    }
}

/// Metadata for an available model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCard {
    pub name: String,
    pub description: String,
    pub release_date: String,
}

/// The Python SDK calls model cards `ModelMetadata`.
pub type ModelMetadata = ModelCard;

/// Models available to the account, with raw HTTP metadata.
#[derive(Clone, Debug, Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelCard>,
    #[serde(skip)]
    pub raw_http_response: RawResponse,
}

impl ListModelsResponse {
    /// Parse a model list and retain HTTP metadata. Non-2xx statuses are errors.
    pub fn from_raw(raw: RawResponse) -> Result<Self, Error> {
        let body = parse_response_body(&raw)?;
        let model_bodies = body
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response(&raw, "models"))?;
        let mut models = Vec::with_capacity(model_bodies.len());
        for (index, value) in model_bodies.iter().enumerate() {
            models.push(ModelCard {
                name: required(value, "name", &format!("models[{index}].name"), &raw)?,
                description: required(
                    value,
                    "description",
                    &format!("models[{index}].description"),
                    &raw,
                )?,
                release_date: required(
                    value,
                    "release_date",
                    &format!("models[{index}].release_date"),
                    &raw,
                )?,
            });
        }
        Ok(Self {
            models,
            raw_http_response: raw,
        })
    }

    pub fn request_id(&self) -> Option<&str> {
        self.raw_http_response.request_id()
    }
}

fn parse_response_body(raw: &RawResponse) -> Result<Value, Error> {
    if !raw.status.is_success() {
        let body = if raw.body.is_empty() {
            Value::Null
        } else {
            raw.json()
                .unwrap_or_else(|_| Value::String(raw.text().into_owned()))
        };
        return Err(crate::ApiError::new(raw.status, body, raw.headers.clone(), None).into());
    }
    raw.json().map_err(|_| invalid_response(raw, "$"))
}

fn required<T: DeserializeOwned>(
    body: &Value,
    key: &str,
    path: &str,
    raw: &RawResponse,
) -> Result<T, Error> {
    let value = body.get(key).ok_or_else(|| invalid_response(raw, path))?;
    serde_json::from_value(value.clone()).map_err(|_| invalid_response(raw, path))
}

fn optional<T: DeserializeOwned>(
    body: &Value,
    key: &str,
    path: &str,
    raw: &RawResponse,
) -> Result<Option<T>, Error> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| invalid_response(raw, path)),
    }
}

fn invalid_response(raw: &RawResponse, field_path: &str) -> Error {
    Error::ResponseValidation {
        field_path: field_path.into(),
        response: Box::new(raw.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(body: Value) -> RawResponse {
        let mut headers = HeaderMap::new();
        headers.insert("x-typesafe-request-id", "req-42".parse().unwrap());
        RawResponse {
            status: StatusCode::OK,
            headers,
            body: serde_json::to_vec(&body).unwrap().into(),
        }
    }

    #[test]
    fn questions_preserve_omitted_null_and_nested_fields() {
        let request = SystemOneRequest::new(
            Value::Null,
            [
                ("omitted", Question::from(Noul::default())),
                ("explicit", Noul::new(Value::Null).null_criteria().into()),
                (
                    "choice",
                    Choice::new([("yes", json!({"example": [null, true]}))]).into(),
                ),
                (
                    "score",
                    Score::new([Value::Null, json!(["high", {"note": null}])]).into(),
                ),
                ("future", json!({"type": "future", "custom": null}).into()),
            ],
        )
        .extra_body([("custom", Value::Null)]);
        assert_eq!(
            request.prepare("jev-latest").unwrap(),
            json!({
                "state": null,
                "model": "jev-latest",
                "custom": null,
                "questions": {
                    "omitted": {"type": "noul"},
                    "explicit": {"type": "noul", "instructions": null, "criteria": null},
                    "choice": {"type": "choice", "criteria": {"yes": {"example": [null, true]}}},
                    "score": {"type": "score", "criteria": [null, ["high", {"note": null}]]},
                    "future": {"type": "future", "custom": null}
                }
            })
        );
    }

    #[test]
    fn invalid_question_structure_is_rejected_before_transport() {
        for question in [
            json!({}),
            json!({"type": ""}),
            json!({"type": 2}),
            json!({"type": "choice"}),
            json!({"type": "choice", "criteria": []}),
            json!({"type": "score", "criteria": []}),
            json!({"type": "score", "criteria": {"0": "bad"}}),
            Value::Null,
        ] {
            assert!(matches!(
                SystemOneRequest::new("state", [("q", question)]).prepare("model"),
                Err(Error::InvalidRequest(_))
            ));
        }
        assert!(SystemOneRequest::new("state", Questions::new())
            .prepare("model")
            .is_err());
        assert!(
            SystemOneRequest::new("state", [("q", Score::new(["only"]))])
                .prepare("model")
                .is_ok()
        );
    }

    #[test]
    fn model_override_and_extra_body_follow_python_merge_semantics() {
        let request = SystemOneRequest::new("state", [("q", Noul::new("question"))])
            .model("override")
            .extra_body([("model", json!("extra")), ("state", Value::Null)]);
        let body = request.prepare("default").unwrap();
        assert_eq!(body["model"], "extra");
        assert!(body["state"].is_null());
    }

    #[test]
    fn typed_answers_preserve_score_levels_and_raw_unknown_answers() {
        let body = json!({"model": "test", "usage": {"input_tokens": 2, "billing_units": 7}, "answers": {
            "spam": {"type": "noul", "noul": 0.9},
            "tone": {"type": "choice", "choice": "calm", "confidence": 0.8, "probabilities": {"calm": 0.8}},
            "quality": {"type": "score", "score": 0.4, "confidence": 0.8,
                "legend": {"0": {"examples": ["low", null]}, "1": null}, "probabilities": {"0": 0.6, "1": 0.4}},
            "future": {"type": "aurora", "value": 3}
        }});
        let response = SystemOneResponse::from_raw(raw(body.clone())).unwrap();
        assert_eq!(response.request_id(), Some("req-42"));
        assert_eq!(response.nouls()["spam"].noul, 0.9);
        assert_eq!(response.choices()["tone"].choice, "calm");
        assert_eq!(response.scores()["quality"].probabilities[&1], 0.4);
        assert_eq!(
            response.scores()["quality"].legend[&0],
            json!({"examples": ["low", null]})
        );
        assert_eq!(response.usage.output_tokens, None);
        assert!(!response.answers.contains_key("future"));
        assert_eq!(response.raw_http_response.json().unwrap(), body);
        let exported = serde_json::to_value(&response).unwrap();
        assert!(exported.get("raw_http_response").is_none());
        assert_eq!(exported["answers"]["spam"]["type"], "noul");
        assert!(exported["usage"].get("billing_units").is_none());
    }

    #[test]
    fn malformed_responses_report_field_paths_and_keep_http_metadata() {
        for (answers, path) in [
            (json!({"n": {"type": "noul"}}), "answers.n.noul"),
            (
                json!({"c": {"type": "choice", "choice": "a", "probabilities": {}}}),
                "answers.c.confidence",
            ),
            (
                json!({"s": {"type": "score", "score": 1, "confidence": 1, "legend": {"x": "bad"}, "probabilities": {}}}),
                "answers.s.legend",
            ),
            (json!({"n": "not an object"}), "answers.n.type"),
        ] {
            let error = SystemOneResponse::from_raw(raw(
                json!({"model": "test", "usage": {}, "answers": answers}),
            ))
            .unwrap_err();
            match error {
                Error::ResponseValidation {
                    field_path,
                    response,
                } => {
                    assert_eq!(field_path, path);
                    assert_eq!(response.request_id(), Some("req-42"));
                }
                error => panic!("unexpected error: {error}"),
            }
        }
    }

    #[test]
    fn model_list_errors_identify_the_nested_missing_field() {
        let error = ListModelsResponse::from_raw(raw(json!({"models": [
            {"name": "test", "description": "Test", "release_date": "2026-09-14"},
            {"name": "test", "release_date": "2026-09-14"}
        ]})))
        .unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "models[1].description")
            }
            error => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn public_parsers_reject_error_statuses_even_with_success_shaped_bodies() {
        let mut response = raw(json!({"model": "test", "usage": {}, "answers": {}, "models": []}));
        response.status = StatusCode::BAD_REQUEST;
        for error in [
            SystemOneResponse::from_raw(response.clone()).unwrap_err(),
            ListModelsResponse::from_raw(response).unwrap_err(),
        ] {
            assert!(matches!(error, Error::Api(_)));
            assert_eq!(error.status(), Some(StatusCode::BAD_REQUEST));
            assert_eq!(error.request_id(), Some("req-42"));
        }
    }
}
