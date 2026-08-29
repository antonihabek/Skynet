//! HTTP/1.1, by hand and only as much of it as §33.7 needs.
//!
//! The workspace has no third-party dependencies (`Cargo.toml` says why), so this is written rather
//! than pulled in. That is a smaller undertaking than it sounds, because the server this backs is
//! not a web server: it answers a handful of REST shapes to a CLI on a Unix socket and to a static
//! console on the LAN, both of which are clients we ship.
//!
//! What is implemented: the request line, headers, `Content-Length` bodies, and a response writer.
//! What is not, and is refused rather than half-supported:
//!
//! * **chunked transfer encoding** — a request that asks for it is answered `411`, because a body
//!   whose length nobody declared is a body whose size nobody bounded;
//! * **keep-alive pipelining** — one request per connection, `Connection: close`. A robot's console
//!   makes a handful of requests a second and the concurrency this saves is not worth a state
//!   machine that can desynchronise two requests;
//! * **anything with a body over [`MAX_BODY`]** — answered `413` before it is read.
//!
//! # Every limit is a refusal, not a truncation
//!
//! A parser that silently drops a header it did not have room for is a parser that can be made to
//! forget an `Authorization`. So every bound here produces an error the caller turns into a status
//! code, and nothing is ever partially accepted.

use std::collections::BTreeMap;

/// The largest request line plus headers this will read.
pub const MAX_HEAD: usize = 8 * 1024;

/// The largest body. Generous for a grant and far below anything that could exhaust a robot.
pub const MAX_BODY: usize = 256 * 1024;

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `POST`, `DELETE`.
    pub method: String,
    /// The path, percent-decoded, with the query removed.
    pub path: String,
    /// The query string as given, undecoded and unparsed.
    pub query: String,
    /// Headers, lowercased.
    pub headers: BTreeMap<String, String>,
    /// The body.
    pub body: Vec<u8>,
}

impl Request {
    /// A header, by lowercase name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The path split on `/`, with empty segments dropped.
    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }

    /// One query parameter, percent-decoded.
    pub fn param(&self, name: &str) -> Option<String> {
        for pair in self.query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if percent_decode(key).as_deref() == Some(name) {
                return percent_decode(value);
            }
        }
        None
    }
}

/// Why a request could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The head exceeded [`MAX_HEAD`], or the body [`MAX_BODY`].
    TooLarge,
    /// The request line or a header was not well formed.
    Malformed,
    /// A body arrived with no `Content-Length`.
    LengthRequired,
    /// The connection ended before the request did.
    Incomplete,
}

impl ParseError {
    /// The status this becomes.
    pub fn status(self) -> u16 {
        match self {
            Self::TooLarge => 413,
            Self::Malformed | Self::Incomplete => 400,
            Self::LengthRequired => 411,
        }
    }
}

/// Where the head ends, if it has ended.
pub fn head_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| at + 4)
}

/// How long a body the head declares.
///
/// `Err` for a head asking for chunked encoding or carrying a `Content-Length` that is not a
/// number, so a caller can answer before reading anything.
pub fn declared_length(head: &str) -> Result<usize, ParseError> {
    let mut length = 0usize;
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => {
                length = value.trim().parse().map_err(|_| ParseError::Malformed)?;
            }
            "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => {
                return Err(ParseError::LengthRequired);
            }
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err(ParseError::TooLarge);
    }
    Ok(length)
}

/// Parse a complete request: the head as text, and the body already read.
pub fn parse(head: &str, body: Vec<u8>) -> Result<Request, ParseError> {
    if head.len() > MAX_HEAD {
        return Err(ParseError::TooLarge);
    }
    let mut lines = head.lines();
    let start = lines.next().ok_or(ParseError::Incomplete)?;
    let mut parts = start.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::Malformed);
    };
    if !version.starts_with("HTTP/1.") || parts.next().is_some() {
        return Err(ParseError::Malformed);
    }
    if !method.bytes().all(|b| b.is_ascii_uppercase()) || method.is_empty() {
        return Err(ParseError::Malformed);
    }

    let (raw_path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = percent_decode(raw_path).ok_or(ParseError::Malformed)?;
    // A path that is not absolute is a proxy request or a malformed one, and neither is something a
    // robot's local API should try to interpret.
    if !path.starts_with('/') {
        return Err(ParseError::Malformed);
    }
    // `..` never reaches routing. Nothing here maps a path onto the filesystem today, and that is
    // exactly the kind of thing a later handler adds without re-reading the parser.
    if path.split('/').any(|segment| segment == "..") {
        return Err(ParseError::Malformed);
    }

    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(ParseError::Malformed)?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.contains(' ') {
            return Err(ParseError::Malformed);
        }
        // Last wins would let a second `Authorization` override the first. Neither does: a
        // duplicated header is a request nobody meant to send.
        if headers.insert(name, value.trim().to_string()).is_some() {
            return Err(ParseError::Malformed);
        }
    }

    Ok(Request {
        method: method.to_string(),
        path,
        query: query.to_string(),
        headers,
        body,
    })
}

/// Percent-decode, rejecting anything that is not valid UTF-8 or a well-formed escape.
///
/// `None` rather than a lossy string: a path that decoded to a replacement character would compare
/// unequal to every route and produce a `404` naming a path nobody sent.
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = bytes.get(index + 1..index + 3)?;
                let text = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(text, 16).ok()?);
                index += 3;
            }
            b'+' => {
                // Only meaningful in a query; in a path a `+` is a literal `+`. Decoding it as a
                // space in both places is the classic way one component's idea of a name stops
                // matching another's, so it is left alone everywhere.
                out.push(b'+');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A response, ready to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Content type.
    pub content_type: String,
    /// Body.
    pub body: Vec<u8>,
    /// Extra headers.
    pub extra: Vec<(String, String)>,
}

impl Response {
    /// A JSON response.
    pub fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8".to_string(),
            body: body.into(),
            extra: Vec::new(),
        }
    }

    /// An HTML response, for the console.
    pub fn html(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8".to_string(),
            body: body.into(),
            extra: Vec::new(),
        }
    }

    /// Add a header.
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.extra.push((name.to_string(), value.to_string()));
        self
    }

    /// The bytes to send.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 256);
        out.extend_from_slice(
            format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status)).as_bytes(),
        );
        out.extend_from_slice(format!("Content-Type: {}\r\n", self.content_type).as_bytes());
        out.extend_from_slice(format!("Content-Length: {}\r\n", self.body.len()).as_bytes());
        // The console is served to a browser on the LAN and answers questions about a machine that
        // can move. Nothing about it should be embeddable, sniffable, or cached into staleness.
        out.extend_from_slice(b"Connection: close\r\n");
        out.extend_from_slice(b"X-Content-Type-Options: nosniff\r\n");
        out.extend_from_slice(b"Cache-Control: no-store\r\n");
        out.extend_from_slice(b"Referrer-Policy: no-referrer\r\n");
        out.extend_from_slice(b"X-Frame-Options: DENY\r\n");
        for (name, value) in &self.extra {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

/// The reason phrase for a status.
pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}
