use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn object() -> Self {
        Self::Object(Vec::new())
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.push(key, value);
        self
    }

    pub fn push(&mut self, key: &str, value: impl Into<Value>) {
        if let Self::Object(entries) = self {
            entries.push((key.to_owned(), value.into()));
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find_map(|(name, value)| (name == key).then_some(value)),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "a boolean",
            Self::Number(_) => "a number",
            Self::String(_) => "a string",
            Self::Array(_) => "an array",
            Self::Object(_) => "an object",
        }
    }

    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        const INLINE_WIDTH: usize = 96;
        let compact = self.to_compact();
        let empty = match self {
            Self::Array(items) => items.is_empty(),
            Self::Object(entries) => entries.is_empty(),
            _ => true,
        };
        if empty || indent * 2 + compact.len() <= INLINE_WIDTH && !self.has_nested_container() {
            out.push_str(&compact);
            return;
        }
        let pad = "  ".repeat(indent + 1);
        match self {
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&pad);
                    item.write_pretty(out, indent + 1);
                }
                out.push('\n');
                out.push_str(&"  ".repeat(indent));
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&pad);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write_pretty(out, indent + 1);
                }
                out.push('\n');
                out.push_str(&"  ".repeat(indent));
                out.push('}');
            }
            _ => unreachable!("scalars are written inline"),
        }
    }

    fn has_nested_container(&self) -> bool {
        let deep = |value: &Value| match value {
            Self::Array(items) => items
                .iter()
                .any(|item| matches!(item, Self::Array(_) | Self::Object(_))),
            Self::Object(entries) => entries
                .iter()
                .any(|(_, item)| matches!(item, Self::Array(_) | Self::Object(_))),
            _ => false,
        };
        match self {
            Self::Array(items) => items.iter().any(deep),
            Self::Object(entries) => entries.iter().any(|(_, value)| deep(value)),
            _ => false,
        }
    }

    pub fn to_compact(&self) -> String {
        let mut out = String::new();
        self.write_compact(&mut out);
        out
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(text) => out.push_str(text),
            Self::String(text) => write_string(out, text),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    write_string(out, key);
                    out.push_str(": ");
                    value.write_compact(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

macro_rules! integer_values {
    ($($kind:ty),*) => {$(
        impl From<$kind> for Value {
            fn from(value: $kind) -> Self {
                Self::Number(value.to_string())
            }
        }
    )*};
}
integer_values!(u8, u16, u32, u64, usize, i64);

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        if value.is_finite() {
            Self::Number(float_text(value.to_string()))
        } else {
            Self::Null
        }
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        if value.is_finite() {
            Self::Number(float_text(value.to_string()))
        } else {
            Self::Null
        }
    }
}

fn float_text(text: String) -> String {
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Self::Array(items.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

const MAX_DEPTH: usize = 128;

pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser {
        text: text.as_bytes(),
        position: 0,
    };
    if parser.text.starts_with(&[0xEF, 0xBB, 0xBF]) {
        parser.position = 3;
    }
    parser.whitespace();
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.position != parser.text.len() {
        return Err(parser.error("unexpected text after the end of the JSON value"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> ParseError {
        let before = &self.text[..self.position.min(self.text.len())];
        let line = before.iter().filter(|&&byte| byte == b'\n').count() + 1;
        let line_start = before
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |index| index + 1);
        let column = String::from_utf8_lossy(&before[line_start..])
            .chars()
            .count()
            + 1;
        ParseError {
            line,
            column,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.position).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, byte: u8, what: &str) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected {what}")))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, ParseError> {
        if self.text[self.position..].starts_with(word.as_bytes()) {
            self.position += word.len();
            Ok(value)
        } else {
            Err(self.error("expected a value"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting is too deep"));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("expected a value")),
            None => Err(self.error("unexpected end of file")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.position += 1;
        let mut entries: Vec<(String, Value)> = Vec::new();
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Value::Object(entries));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a quoted key"));
            }
            let key_position = self.position;
            let key = self.string()?;
            if entries.iter().any(|(name, _)| *name == key) {
                self.position = key_position;
                return Err(self.error(format!("duplicate key \"{key}\"")));
            }
            self.whitespace();
            self.expect(b':', "':' after the key")?;
            self.whitespace();
            let value = self.value(depth + 1)?;
            entries.push((key, value));
            self.whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                    self.whitespace();
                    if self.peek() == Some(b'}') {
                        return Err(self.error("trailing comma before '}'"));
                    }
                }
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Value::Object(entries));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.position += 1;
        let mut items = Vec::new();
        self.whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.whitespace();
            items.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                    self.whitespace();
                    if self.peek() == Some(b']') {
                        return Err(self.error("trailing comma before ']'"));
                    }
                }
                Some(b']') => {
                    self.position += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.position += 1;
        let mut bytes = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => {
                    self.position += 1;
                    return String::from_utf8(bytes)
                        .map_err(|_| self.error("string is not valid UTF-8"));
                }
                Some(b'\\') => {
                    self.position += 1;
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated string"))?;
                    self.position += 1;
                    let character = match escaped {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => {
                            self.position -= 1;
                            return Err(self.error("unknown escape sequence"));
                        }
                    };
                    let mut buffer = [0; 4];
                    bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
                Some(byte) if byte < 0x20 => {
                    return Err(self.error("control character in string; use an escape"));
                }
                Some(byte) => {
                    bytes.push(byte);
                    self.position += 1;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self
            .text
            .get(self.position..self.position + 4)
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .ok_or_else(|| self.error("expected four hex digits after \\u"))?;
        self.position += 4;
        Ok(digits)
    }

    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let first = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&first) {
            if !self.text[self.position..].starts_with(b"\\u") {
                return Err(self.error("unpaired surrogate in \\u escape"));
            }
            self.position += 2;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(self.error("unpaired surrogate in \\u escape"));
            }
            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
        } else {
            first
        };
        char::from_u32(code).ok_or_else(|| self.error("invalid \\u escape"))
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.position;
        let digits = |parser: &mut Self| {
            let begin = parser.position;
            while matches!(parser.peek(), Some(b'0'..=b'9')) {
                parser.position += 1;
            }
            parser.position > begin
        };
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        if self.peek() == Some(b'0') {
            self.position += 1;
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("numbers cannot have leading zeros"));
            }
        } else if !digits(self) {
            return Err(self.error("expected digits"));
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            if !digits(self) {
                return Err(self.error("expected digits after the decimal point"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !digits(self) {
                return Err(self.error("expected digits in the exponent"));
            }
        }
        let text = std::str::from_utf8(&self.text[start..self.position])
            .expect("number characters are ASCII");
        Ok(Value::Number(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_with_exact_numbers_and_key_order() {
        let value = Value::object()
            .with("tick", u64::MAX)
            .with("gain", 0.1_f32)
            .with("bpm", 120.0_f64)
            .with("tiny", f64::from_bits(1))
            .with("name", "Kick \"808\"\n\u{1}é")
            .with("notes", vec![Value::object().with("key", 60_u8)])
            .with("none", Value::Null);
        let text = value.to_pretty();
        assert_eq!(parse(&text).unwrap(), value);
        let Value::Number(gain) = value.get("gain").unwrap() else {
            panic!()
        };
        assert_eq!(gain, "0.1");
        assert_eq!(gain.parse::<f32>().unwrap(), 0.1_f32);
        assert!(text.find("\"tick\"").unwrap() < text.find("\"gain\"").unwrap());
        assert!(text.contains("\"bpm\": 120.0"));
    }

    #[test]
    fn short_containers_stay_on_one_line() {
        let notes: Vec<Value> = (0..3)
            .map(|index| {
                Value::object()
                    .with("start", index * 240_u64)
                    .with("length", 240_u64)
                    .with("key", 60_u8)
            })
            .collect();
        let text = Value::object().with("notes", notes).to_pretty();
        assert_eq!(
            text,
            "{\n  \"notes\": [\n    {\"start\": 0, \"length\": 240, \"key\": 60},\n    \
             {\"start\": 240, \"length\": 240, \"key\": 60},\n    \
             {\"start\": 480, \"length\": 240, \"key\": 60}\n  ]\n}\n"
        );
    }

    #[test]
    fn errors_name_the_line_and_column() {
        let error = parse("{\n  \"a\": 1,\n  \"b\": [1, 2,]\n}").unwrap_err();
        assert_eq!((error.line, error.column), (3, 14));
        assert!(error.message.contains("trailing comma"));
        assert!(
            parse("{\"a\": 1, \"a\": 2}")
                .unwrap_err()
                .message
                .contains("duplicate")
        );
        assert!(parse("[01]").is_err());
        assert!(parse("[1.]").is_err());
        assert!(parse("\"\\ud800\"").is_err());
        assert_eq!(
            parse("\"\\ud83c\\udfb5\"").unwrap(),
            Value::String("🎵".into())
        );
        assert!(parse("[1] x").is_err());
        assert!(
            parse(&"[".repeat(500))
                .unwrap_err()
                .message
                .contains("deep")
        );
    }
}
