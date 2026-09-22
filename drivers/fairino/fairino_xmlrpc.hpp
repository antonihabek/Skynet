/* The XML-RPC half of the FR wire: enough of it, and no more.
 *
 * FAIRINO's controller answers XML-RPC over HTTP/1.1 on port 20003 and every command verb in the
 * vendor's SDK goes through it (`libfairino/src/src/Robot/robot.cpp`, one
 * `XmlRpcClient c(serverUrl, 20003)` per verb). XML-RPC++ — the library the SDK vendors to speak
 * it — is 4,000 lines that also do server dispatch, multicall, base64, structs, an event loop and
 * a logger. This driver needs four value shapes and two message shapes, so it carries those and
 * says which parts of the specification it does not implement, rather than vendoring a library
 * into a repository whose licence would then have to be reasoned about at every build.
 *
 * ── What is here, and what is deliberately not ───────────────────────────
 *
 *   values     `<i4>`/`<int>`, `<double>`, `<boolean>`, `<string>` (tagged and bare, because
 *              XmlRpc++'s `stringToXml` emits a bare `<value>text</value>`) and `<array>`.
 *   messages   `<methodCall>` with `<params>`, and `<methodResponse>` with either `<params>` or
 *              `<fault>`.
 *   not here   `<struct>`, `<base64>`, `<dateTime.iso8601>`, `system.multicall`. Nothing FAIRINO
 *              sends or expects uses them: every parameter in `robot.cpp` is a number, a string
 *              or an array of numbers. A document that used one arrives here as a parse failure,
 *              which is the honest outcome — a partial parse that guessed would be worse.
 *
 * The HTTP header this builds is the one XmlRpc++'s own `generateHeader` builds, down to the
 * `Content-length: ` spelling, because its `readHeader` finds the body with a
 * `strncasecmp(cp, "Content-length: ", 16)` and nothing else. The fake controller answers with
 * the same spelling for the same reason: `cross_check.cpp` puts the vendor's client on the other
 * end of it.
 *
 * Licence: Apache-2.0 (this file). Written from the XML-RPC specification and from the shapes
 * `robot.cpp` sends; no XmlRpc++ code is copied here.
 */

#ifndef SKYNET_FAIRINO_XMLRPC_HPP
#define SKYNET_FAIRINO_XMLRPC_HPP

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <strings.h>
#include <string>
#include <string_view>
#include <vector>

namespace skynet_fairino {

struct RpcValue {
  enum class Kind { Int, Double, Boolean, String, Array };

  Kind kind = Kind::Int;
  long integer = 0;
  double number = 0.0;
  std::string text;
  std::vector<RpcValue> items;

  static RpcValue of_int(long value) {
    RpcValue v;
    v.kind = Kind::Int;
    v.integer = value;
    return v;
  }
  static RpcValue of_double(double value) {
    RpcValue v;
    v.kind = Kind::Double;
    v.number = value;
    return v;
  }
  static RpcValue of_string(std::string value) {
    RpcValue v;
    v.kind = Kind::String;
    v.text = std::move(value);
    return v;
  }
  static RpcValue of_array(std::vector<RpcValue> values) {
    RpcValue v;
    v.kind = Kind::Array;
    v.items = std::move(values);
    return v;
  }
  static RpcValue of_doubles(const double *values, std::size_t count) {
    std::vector<RpcValue> items;
    items.reserve(count);
    for (std::size_t i = 0; i < count; ++i) items.push_back(of_double(values[i]));
    return of_array(std::move(items));
  }

  /* Every shape this driver reads back is either an integer or an array whose first element is
   * one — the vendor's own `int(result)` and `int(result[0])`. */
  bool as_int(long *out) const {
    if (kind == Kind::Int || kind == Kind::Boolean) {
      *out = integer;
      return true;
    }
    if (kind == Kind::Double) {
      *out = static_cast<long>(number);
      return true;
    }
    return false;
  }
  bool as_double(double *out) const {
    if (kind == Kind::Double) {
      *out = number;
      return true;
    }
    if (kind == Kind::Int || kind == Kind::Boolean) {
      *out = static_cast<double>(integer);
      return true;
    }
    return false;
  }

  std::string to_xml() const;
};

inline std::string xml_escape(std::string_view raw) {
  std::string out;
  out.reserve(raw.size());
  for (const char c : raw) {
    switch (c) {
      case '<': out += "&lt;"; break;
      case '>': out += "&gt;"; break;
      case '&': out += "&amp;"; break;
      default: out += c; break;
    }
  }
  return out;
}

inline std::string RpcValue::to_xml() const {
  switch (kind) {
    case Kind::Int: {
      char buffer[32];
      std::snprintf(buffer, sizeof buffer, "<value><i4>%ld</i4></value>", integer);
      return buffer;
    }
    case Kind::Boolean: {
      char buffer[48];
      std::snprintf(buffer, sizeof buffer, "<value><boolean>%d</boolean></value>",
                    integer ? 1 : 0);
      return buffer;
    }
    case Kind::Double: {
      /* Six decimals is XmlRpc++'s own default format (`%f`), and on this wire the units are
       * degrees and newton-metres — a micro-degree is four orders of magnitude finer than the
       * arm's repeatability. */
      char buffer[64];
      std::snprintf(buffer, sizeof buffer, "<value><double>%f</double></value>", number);
      return buffer;
    }
    case Kind::String:
      return "<value><string>" + xml_escape(text) + "</string></value>";
    case Kind::Array: {
      std::string out = "<value><array><data>";
      for (const RpcValue &item : items) out += item.to_xml();
      out += "</data></array></value>";
      return out;
    }
  }
  return "<value><i4>0</i4></value>";
}

/* ── parsing ──────────────────────────────────────────────────────────── */

inline void rpc_skip_space(const std::string &xml, std::size_t *at) {
  while (*at < xml.size() && (xml[*at] == ' ' || xml[*at] == '\t' || xml[*at] == '\r' ||
                              xml[*at] == '\n')) {
    ++*at;
  }
}

inline bool rpc_eat(const std::string &xml, std::size_t *at, const char *tag) {
  rpc_skip_space(xml, at);
  const std::size_t len = std::strlen(tag);
  if (xml.compare(*at, len, tag) != 0) return false;
  *at += len;
  return true;
}

bool rpc_parse_value(const std::string &xml, std::size_t *at, RpcValue *out);

inline bool rpc_parse_array(const std::string &xml, std::size_t *at, RpcValue *out) {
  if (!rpc_eat(xml, at, "<data>")) return false;
  out->kind = RpcValue::Kind::Array;
  for (;;) {
    rpc_skip_space(xml, at);
    if (rpc_eat(xml, at, "</data>")) break;
    RpcValue item;
    if (!rpc_parse_value(xml, at, &item)) return false;
    out->items.push_back(std::move(item));
    if (out->items.size() > 4096) return false;
  }
  return rpc_eat(xml, at, "</array>");
}

inline bool rpc_parse_value(const std::string &xml, std::size_t *at, RpcValue *out) {
  /* Overwrite, never append. A parser that left the caller's previous array in place would read
   * one call's reply as the tail of the last one's — which is exactly the bug this line fixes,
   * found by the suite's check that a serial number is a serial number and not the model name
   * left over from the query before it. */
  *out = RpcValue{};
  if (!rpc_eat(xml, at, "<value>")) return false;
  rpc_skip_space(xml, at);
  if (rpc_eat(xml, at, "<array>")) {
    if (!rpc_parse_array(xml, at, out)) return false;
    return rpc_eat(xml, at, "</value>");
  }
  const char *const number_tags[] = {"<i4>", "<int>", "<double>", "<boolean>"};
  const char *const close_tags[] = {"</i4>", "</int>", "</double>", "</boolean>"};
  for (int i = 0; i < 4; ++i) {
    if (!rpc_eat(xml, at, number_tags[i])) continue;
    const std::size_t end = xml.find(close_tags[i], *at);
    if (end == std::string::npos) return false;
    const std::string body = xml.substr(*at, end - *at);
    *at = end + std::strlen(close_tags[i]);
    if (i == 2) {
      out->kind = RpcValue::Kind::Double;
      out->number = std::strtod(body.c_str(), nullptr);
    } else {
      out->kind = i == 3 ? RpcValue::Kind::Boolean : RpcValue::Kind::Int;
      out->integer = std::strtol(body.c_str(), nullptr, 10);
    }
    return rpc_eat(xml, at, "</value>");
  }
  if (rpc_eat(xml, at, "<string>")) {
    const std::size_t end = xml.find("</string>", *at);
    if (end == std::string::npos) return false;
    out->kind = RpcValue::Kind::String;
    out->text = xml.substr(*at, end - *at);
    *at = end + 9;
    return rpc_eat(xml, at, "</value>");
  }
  /* A bare `<value>text</value>`, which is what XmlRpc++'s `stringToXml` emits. */
  const std::size_t end = xml.find("</value>", *at);
  if (end == std::string::npos) return false;
  out->kind = RpcValue::Kind::String;
  out->text = xml.substr(*at, end - *at);
  *at = end + 8;
  return true;
}

/* ── messages ─────────────────────────────────────────────────────────── */

inline std::string rpc_request_body(const std::string &method,
                                    const std::vector<RpcValue> &params) {
  std::string body = "<?xml version=\"1.0\"?>\r\n<methodCall><methodName>";
  body += method;
  body += "</methodName>\r\n<params>";
  for (const RpcValue &param : params) {
    body += "<param>";
    body += param.to_xml();
    body += "</param>";
  }
  body += "</params></methodCall>\r\n";
  return body;
}

inline std::string rpc_request(const std::string &host, unsigned port, const std::string &method,
                               const std::vector<RpcValue> &params) {
  const std::string body = rpc_request_body(method, params);
  char header[256];
  std::snprintf(header, sizeof header,
                "POST /RPC2 HTTP/1.1\r\nUser-Agent: skynet-fairino\r\nHost: %s:%u\r\n"
                "Content-Type: text/xml\r\nContent-length: %zu\r\n\r\n",
                host.c_str(), port, body.size());
  return std::string(header) + body;
}

inline bool rpc_parse_request(const std::string &body, std::string *method,
                              std::vector<RpcValue> *params) {
  const std::size_t name_at = body.find("<methodName>");
  if (name_at == std::string::npos) return false;
  const std::size_t name_end = body.find("</methodName>", name_at);
  if (name_end == std::string::npos) return false;
  *method = body.substr(name_at + 12, name_end - name_at - 12);

  std::size_t at = body.find("<params>", name_end);
  if (at == std::string::npos) return true;  /* a verb with no parameters is legal */
  at += 8;
  for (;;) {
    rpc_skip_space(body, &at);
    if (rpc_eat(body, &at, "</params>")) break;
    if (!rpc_eat(body, &at, "<param>")) return false;
    RpcValue value;
    if (!rpc_parse_value(body, &at, &value)) return false;
    if (!rpc_eat(body, &at, "</param>")) return false;
    params->push_back(std::move(value));
    if (params->size() > 64) return false;
  }
  return true;
}

inline std::string rpc_response(const RpcValue &value) {
  const std::string body = "<?xml version=\"1.0\"?>\r\n<methodResponse><params><param>" +
                           value.to_xml() + "</param></params></methodResponse>\r\n";
  char header[192];
  std::snprintf(header, sizeof header,
                "HTTP/1.1 200 OK\r\nServer: skynet-fairino-fake\r\nContent-Type: text/xml\r\n"
                "Content-length: %zu\r\n\r\n",
                body.size());
  return std::string(header) + body;
}

/* A `<fault>`, which the vendor's client reports as *"method may not exist"* and turns into
 * `ERR_XMLRPC_CMD_FAILED`. The fake answers an unknown verb with one rather than with a zero,
 * because a fake that answered "fine" to a verb it had never heard of would let a driver pass a
 * conformance run by talking to nothing. */
inline std::string rpc_response_fault(int code, const std::string &message) {
  char buffer[512];
  std::snprintf(buffer, sizeof buffer,
                "<?xml version=\"1.0\"?>\r\n<methodResponse><fault><value><array><data>"
                "<value><i4>%d</i4></value><value><string>%s</string></value>"
                "</data></array></value></fault></methodResponse>\r\n",
                code, xml_escape(message).c_str());
  const std::string body = buffer;
  char header[192];
  std::snprintf(header, sizeof header,
                "HTTP/1.1 200 OK\r\nServer: skynet-fairino-fake\r\nContent-Type: text/xml\r\n"
                "Content-length: %zu\r\n\r\n",
                body.size());
  return std::string(header) + body;
}

/* The reply as the vendor reads it: `<fault>` is a failure, `<params><param>` carries the value,
 * and the value is either the error code or an array beginning with it. */
inline bool rpc_parse_response(const std::string &body, RpcValue *out, bool *faulted) {
  *faulted = false;
  const std::size_t response_at = body.find("<methodResponse>");
  if (response_at == std::string::npos) return false;
  std::size_t at = response_at + 16;
  rpc_skip_space(body, &at);
  if (rpc_eat(body, &at, "<fault>")) {
    *faulted = true;
    return rpc_parse_value(body, &at, out);
  }
  if (!rpc_eat(body, &at, "<params>")) return false;
  if (!rpc_eat(body, &at, "<param>")) return false;
  return rpc_parse_value(body, &at, out);
}

/* The body's length, from the header XmlRpc++ writes and this file copies. Returns false while
 * the header is still incomplete. */
inline bool rpc_content_length(const std::string &buffer, std::size_t *header_bytes,
                               std::size_t *content_length) {
  const std::size_t end = buffer.find("\r\n\r\n");
  if (end == std::string::npos) return false;
  *header_bytes = end + 4;
  const std::string header = buffer.substr(0, end);
  for (std::size_t i = 0; i + 16 <= header.size(); ++i) {
    if (strncasecmp(header.c_str() + i, "Content-length: ", 16) == 0) {
      *content_length = static_cast<std::size_t>(std::strtoul(header.c_str() + i + 16, nullptr, 10));
      return true;
    }
  }
  *content_length = 0;
  return true;
}

}  // namespace skynet_fairino

#endif  // SKYNET_FAIRINO_XMLRPC_HPP
