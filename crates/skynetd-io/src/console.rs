//! The on-robot console (§32.2).
//!
//! > Served by `skynetd` over the LAN. Small, static, no build step, no CDN — it has to render on a
//! > robot with no internet, which rules out every fashionable way to build it.
//!
//! Three files, compiled into the binary by [`include_str!`]. Not read from disk, and the reason is
//! §27's: the console is what an operator reaches for when something is wrong, and a page that
//! could be missing because a partition failed to mount is a page that is missing exactly when it
//! is needed. A binary that started has its console.
//!
//! # Why three files and not one
//!
//! A single self-contained page would need inline `<script>` and inline `<style>`, and a
//! `Content-Security-Policy` that permits those permits every injection this console's own escaping
//! discipline is there to prevent. Split into three, the policy is `script-src 'self'` with no
//! `unsafe-inline` anywhere — which costs two extra requests on a link-local socket and buys a
//! header that is actually restrictive.
//!
//! It is still no build step. Three files copied into a binary is not a toolchain.
//!
//! # What is served, and to whom
//!
//! All three are [`crate::auth::Role::Anonymous`], like `GET /status` and `POST /stop`.
//!
//! That is deliberate and it is the same argument §39 makes about the stop button: the page carries
//! no robot state — every number on it arrives later, from a route that enforces its own role — and
//! a console you must authenticate to *load* is a console whose stop button is behind a login form.
//! §39.2's "an unprovisioned robot's console offers status and stop and nothing else" describes
//! exactly what this page does when no token is held: it renders, the two anonymous panels fill in,
//! and every other panel reports that it needs one.

use crate::http::Response;

/// The page.
const HTML: &str = include_str!("../console/console.html");
/// Its stylesheet.
const CSS: &str = include_str!("../console/console.css");
/// Its script.
const JS: &str = include_str!("../console/console.js");

/// The policy the console is served under.
///
/// `default-src 'none'` and then only what the page actually uses. Worth reading as a list of
/// things this console is structurally unable to do:
///
/// * `script-src 'self'` — no inline script, no `eval`, and nothing from anywhere else. There is no
///   CDN to fall back to and no origin but this one.
/// * `connect-src 'self'` — the page can talk to the daemon that served it and to nothing else. A
///   console on a robot that phones home is the §34.2 failure in a different costume.
/// * `img-src 'none'`, `font-src 'none'` — there are no images and no web font. A robot with no
///   internet renders this page identically to one with.
/// * `form-action 'none'`, `base-uri 'none'`, `frame-ancestors 'none'` — nothing to submit
///   anywhere, no way to rewrite what a relative URL means, and not embeddable.
///
/// There is deliberately no `blob:` anywhere in it. Downloading a sealed recording fetches the
/// bytes from this origin — which `connect-src 'self'` covers — and only then wraps them in an
/// object URL to give the browser a filename. The bytes never come from a blob; a blob is what they
/// are put into on the way out, and a directive permitting one would be permitting something this
/// page does not do.
pub const CSP: &str = "default-src 'none'; \
                       script-src 'self'; \
                       style-src 'self'; \
                       connect-src 'self'; \
                       img-src 'none'; \
                       font-src 'none'; \
                       form-action 'none'; \
                       base-uri 'none'; \
                       frame-ancestors 'none'";

/// Whether this method and path is a console asset, and what to answer with.
///
/// Returns `None` for anything else, so [`crate::api::handle`] falls through to the API routes
/// unchanged. Only `GET` — a console asset is not something to POST to, and letting the method
/// through would make `POST /` a 200.
pub fn asset(method: &str, segments: &[&str], theme: crate::settings::Theme) -> Option<Response> {
    if method != "GET" {
        return None;
    }
    let response = match segments {
        // The empty path. `Request::segments` drops empty components, so `/` arrives as no
        // segments at all.
        [] => Response::html(200, themed(theme)),
        ["console.css"] => Response {
            status: 200,
            content_type: "text/css; charset=utf-8".to_string(),
            body: CSS.as_bytes().to_vec(),
            extra: Vec::new(),
        },
        ["console.js"] => Response {
            status: 200,
            content_type: "text/javascript; charset=utf-8".to_string(),
            body: JS.as_bytes().to_vec(),
            extra: Vec::new(),
        },
        _ => return None,
    };
    Some(response.with("Content-Security-Policy", CSP))
}

/// The page with §38.1's `console_theme` pinned onto its root element.
///
/// Server-side rather than in the script, and that is the whole reason it is done here at all: a
/// page that fetched its own theme after loading would render in the browser's colours first and
/// swap, and the swap is most visible on the machine the setting was set for — a bench under shop
/// lighting where the workstation is configured dark. A theme that arrives with the HTML has
/// nothing to swap from.
///
/// [`Theme::System`] substitutes nothing at all, so the default path serves the file byte for byte
/// and `prefers-color-scheme` decides, exactly as it did before this setting existed.
fn themed(theme: crate::settings::Theme) -> String {
    match theme {
        crate::settings::Theme::System => HTML.to_string(),
        pinned => HTML.replacen(
            ROOT_ELEMENT,
            &format!("<html lang=\"en\" data-theme=\"{}\">", pinned.as_str()),
            1,
        ),
    }
}

/// The opening tag the theme is substituted into.
///
/// A constant rather than a literal at the call site, and asserted in the suite against the page
/// itself: an edit to `console.html` that reformatted this tag would otherwise make the theme
/// setting silently stop working, with the page still serving perfectly.
const ROOT_ELEMENT: &str = "<html lang=\"en\">";

/// Whether a path is one of the console's, for the authorisation table.
///
/// Kept beside [`asset`] rather than written out again in `auth`, because two lists of the same
/// three paths is one list that will eventually be wrong — and the failure mode is a console asset
/// falling through to `auth`'s catch-all, which requires admin. A robot whose console will not load
/// until you authenticate is the thing §39 spends its opening paragraph refusing.
pub fn is_asset(method: &str, segments: &[&str]) -> bool {
    method == "GET" && matches!(segments, [] | ["console.css"] | ["console.js"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_element_this_module_substitutes_is_the_one_the_page_has() {
        // The check that makes the theme setting fail loudly rather than silently. Reformatting
        // `<html lang="en">` in `console.html` would otherwise leave `themed()` substituting
        // nothing, the page serving perfectly, and the setting quietly doing nothing at all.
        assert_eq!(
            HTML.matches(ROOT_ELEMENT).count(),
            1,
            "console.html must contain exactly one `{ROOT_ELEMENT}` for the theme to pin onto"
        );
    }

    #[test]
    fn the_default_serves_the_page_byte_for_byte() {
        // §38.1's default is `system`, and a robot nobody configured must serve exactly the file
        // that is compiled in — `prefers-color-scheme` decides, as it did before this existed.
        let page = asset("GET", &[], crate::settings::Theme::System).expect("the page");
        assert_eq!(page.body, HTML.as_bytes());
        assert!(!String::from_utf8_lossy(&page.body).contains("data-theme"));
    }

    #[test]
    fn a_pinned_theme_arrives_on_the_root_element_with_the_html() {
        // On the HTML rather than fetched by the script, so there is nothing to swap from. The
        // swap would be most visible on exactly the machine the setting exists for.
        for (theme, word) in [
            (crate::settings::Theme::Light, "light"),
            (crate::settings::Theme::Dark, "dark"),
        ] {
            let page = asset("GET", &[], theme).expect("the page");
            let text = String::from_utf8(page.body).expect("utf-8");
            assert!(
                text.contains(&format!("<html lang=\"en\" data-theme=\"{word}\">")),
                "{word} should be pinned on the root element"
            );
            // And the stylesheet has to have something to do with it, or the attribute is
            // decoration. Both directions: a pin must beat the media query and the bare `:root`.
            assert!(CSS.contains(&format!("html[data-theme=\"{word}\"]")));
        }
    }

    #[test]
    fn the_three_assets_are_served_and_nothing_else_is() {
        assert!(asset("GET", &[], crate::settings::Theme::System).is_some());
        assert!(asset("GET", &["console.css"], crate::settings::Theme::System).is_some());
        assert!(asset("GET", &["console.js"], crate::settings::Theme::System).is_some());

        assert!(
            asset("GET", &["status"], crate::settings::Theme::System).is_none(),
            "the API still routes"
        );
        assert!(asset("GET", &["console.map"], crate::settings::Theme::System).is_none());
        assert!(
            asset("POST", &[], crate::settings::Theme::System).is_none(),
            "a console asset is not something to POST to"
        );
    }

    #[test]
    fn every_asset_carries_the_policy_and_its_own_type() {
        let page = asset("GET", &[], crate::settings::Theme::System).expect("the page");
        assert!(page.content_type.starts_with("text/html"));
        assert!(page
            .extra
            .iter()
            .any(|(name, value)| name == "Content-Security-Policy" && value == CSP));

        let css =
            asset("GET", &["console.css"], crate::settings::Theme::System).expect("the stylesheet");
        assert!(css.content_type.starts_with("text/css"));

        let js = asset("GET", &["console.js"], crate::settings::Theme::System).expect("the script");
        assert!(js.content_type.starts_with("text/javascript"));
    }

    /// The policy is only worth having if it forbids the things the page must not do.
    ///
    /// Asserted rather than eyeballed, because a `Content-Security-Policy` is one string and a
    /// single `unsafe-inline` added to make something work quietly removes the whole point of it.
    #[test]
    fn the_policy_permits_no_inline_script_and_no_other_origin() {
        assert!(CSP.contains("default-src 'none'"));
        assert!(!CSP.contains("unsafe-inline"), "{CSP}");
        assert!(!CSP.contains("unsafe-eval"), "{CSP}");
        assert!(!CSP.contains('*'), "no wildcard origin: {CSP}");
        assert!(CSP.contains("connect-src 'self'"));
        assert!(CSP.contains("frame-ancestors 'none'"));
    }

    /// §32.2: the stop control is never below the fold and never behind a confirm dialog.
    ///
    /// Both are properties of the page's source, and both are the kind of thing a later edit
    /// removes without noticing. The stop button being inside the sticky header is the first; the
    /// script never calling `confirm()` is the second.
    #[test]
    fn the_stop_control_is_in_the_sticky_header_and_asks_nobody_anything() {
        let header = HTML
            .split("<header")
            .nth(1)
            .and_then(|rest| rest.split("</header>").next())
            .expect("the page has a header");
        assert!(
            header.contains("id=\"stop\""),
            "the stop button belongs in the header that stays on screen"
        );
        assert!(
            CSS.contains("position: sticky"),
            "the header has to stay on screen at every scroll position"
        );
        assert!(
            !JS.contains("confirm("),
            "§32.2: never behind a confirm dialog on a page whose purpose is stopping a robot"
        );
    }

    /// The page fetches nothing from anywhere but the robot that served it.
    ///
    /// The CSP already forbids it, but a page that needs the CSP to stop it reaching a CDN is a
    /// page that fails differently on a browser that ignores the header. So the source carries no
    /// absolute URL at all.
    #[test]
    fn nothing_is_loaded_from_off_the_robot() {
        for (name, text) in [("html", HTML), ("css", CSS), ("js", JS)] {
            assert!(
                !text.contains("http://") && !text.contains("https://"),
                "{name} reaches off the robot, and this console renders with no internet"
            );
        }
    }

    /// Every panel on the page is filled by a feed, and every feed has a panel to fill.
    ///
    /// Three files that have to agree about a name, and nothing checked that they did. The failure
    /// is silent in both directions and neither one looks like a bug: a `<section id="panel-x">`
    /// with no `Feed('x', …)` renders an empty box for ever, and a `Feed('x', …)` with no section
    /// polls the robot on every tick and throws the answer away. The `data-age` attribute is the
    /// third spelling — it is what `paintAges` looks the panel up by — so a panel whose age label
    /// carries a different name than its section silently stops showing how stale it is.
    ///
    /// Added when the events panel was written, because writing it meant getting the same name
    /// right in three places by hand.
    #[test]
    fn every_panel_has_a_feed_and_every_feed_has_a_panel() {
        let names = |text: &str, open: &str, close: char| -> Vec<String> {
            text.split(open)
                .skip(1)
                .filter_map(|rest| rest.split(close).next().map(str::to_string))
                .collect()
        };

        let panels = names(HTML, "id=\"panel-", '"');
        let ages = names(HTML, "data-age=\"", '"');
        let feeds = names(JS, "new Feed('", '\'');

        assert!(!panels.is_empty(), "the page has panels");

        // The invariant is stated around the **age label**, and getting there took two wrong
        // attempts worth recording, because each was wrong about a shape the page legitimately has.
        //
        // *"Every panel has a feed"* is false: `panel-access` is a token field — a form, not a view
        // of the robot — and giving it an age would draw a staleness for a value the robot never
        // sent. *"Every feed has a panel"* is false too: `releases` renders into a subpanel of
        // `panel-ota`, because §12's verdict and the pull that produced it are one answer and were
        // deliberately not split into two boxes.
        //
        // What is true of all three files at once is the age: a `data-age` is what `paintAges`
        // looks a panel up by, so it needs a section to find and a feed to have a timestamp. And a
        // panel that is fed needs one, or it silently stops showing how old what it displays is —
        // on a page whose whole rule is that it must never show a value it did not receive (§32.2),
        // an undated panel is the one way left to imply a value is current when it is not.
        for age in &ages {
            assert!(
                panels.contains(age),
                "`data-age=\"{age}\"` labels a panel that is not on the page"
            );
            assert!(
                feeds.contains(age),
                "`data-age=\"{age}\"` dates a panel nothing polls, so the age never changes"
            );
        }
        for panel in &panels {
            // `bodyFeed` and `packagesFeed` are constructed on their own lines and named there;
            // both spell their name in the same `Feed('…'` form, so this finds them too.
            if feeds.contains(panel) {
                assert!(
                    ages.contains(panel),
                    "panel `{panel}` is fed and carries no `data-age`: it will show values without \
                     ever saying how old they are"
                );
            }
        }
    }

    /// §32.2's second constraint, checked where it can be: the script must have the vocabulary for
    /// an absent value, and must not be building markup out of strings.
    ///
    /// The three patterns are the *assigning* forms. Reading `.innerHTML` is harmless and the first
    /// version of this test searched for the bare word — which it duly found, in the comment saying
    /// the script does not use it. Matching what is dangerous is both stricter and immune to the
    /// file describing its own discipline.
    #[test]
    fn the_script_renders_an_absent_value_as_a_reason_and_never_as_markup() {
        assert!(
            JS.contains("function absent("),
            "a value nobody measured needs its own rendering, or it becomes a zero"
        );
        for pattern in [
            "innerHTML =",
            "innerHTML=",
            "insertAdjacentHTML",
            "document.write",
        ] {
            assert!(
                !JS.contains(pattern),
                "`{pattern}`: every value on this page originates outside it, and the cheapest way \
                 to keep it escaped is to never build markup from a string"
            );
        }
    }
}
