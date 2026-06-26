//! Bot-management challenge detection (Cloudflare, DataDome, PerimeterX/HUMAN,
//! Akamai/Imperva) and a retry-with-clearance-cookie loop.
//!
//! Detects and classifies the managed-challenge / JS-check / interactive-captcha
//! interstitials these vendors return, so callers can react — retry with a
//! clearance cookie, surface a structured result, or hand off to an external
//! solver — instead of silently treating the challenge HTML as page content.
//!
//! # Scope — DETECT + RETRY, not auto-solve
//!
//! Auto-*solving* a Cloudflare/DataDome managed challenge means executing the
//! vendor's heavily-obfuscated anti-bot JS, which performs deep environment
//! introspection (canvas, audio, WebRTC, font enumeration, DOM bindings) far
//! beyond what a pure-Rust `boa_engine` runtime can satisfy — even real
//! Puppeteer fails without dedicated stealth plugins. This module therefore:
//!
//! 1. **Detects** the challenge (status + headers + body fingerprint) and
//!    names the cookie a successful solve would set.
//! 2. **Retries** when a challenge is detected (see
//!    [`crate::network::client::HttpClient::fetch_with_challenge_retry`]),
//!    re-sending any clearance cookie the cookie jar captured from a prior
//!    attempt's `Set-Cookie`. A retry only clears the challenge when the passive
//!    stealth tier already satisfies it, or when a clearance cookie
//!    (`cf_clearance`/`datadome`/…) was obtained out-of-band.
//!
//! To clear a managed challenge that the passive tier cannot pass, obtain a
//! clearance cookie from an external solver, inject it into the cookie jar, and
//! re-fetch.

/// The bot-management vendor that issued the challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeVendor {
    Cloudflare,
    DataDome,
    PerimeterX,
    AkamaiImperva,
    Unknown,
}

impl ChallengeVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::DataDome => "datadome",
            Self::PerimeterX => "perimeterx",
            Self::AkamaiImperva => "akamai-imperva",
            Self::Unknown => "unknown",
        }
    }
}

/// The kind of challenge, which determines whether it is retry-clearable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    /// "Just a moment…" / 5-second interstitial — JS computes a token.
    Managed,
    /// Lightweight passive JS check (no visible interstitial).
    JsCheck,
    /// Interactive captcha (Turnstile / hCaptcha / reCAPTCHA) — needs a human.
    Interactive,
    /// Hard 403 with no solvable challenge (e.g. WAF block, bad reputation).
    Blocked,
    /// Present but unclassified.
    Unknown,
}

/// A detected bot-management challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedChallenge {
    pub vendor: ChallengeVendor,
    pub kind: ChallengeKind,
    /// Cookie a successful solve sets and a retry must re-present
    /// (`cf_clearance` / `datadome` / `_px3` / …).
    pub clearance_cookie: &'static str,
    /// Vendor request id for correlation (`cf-ray`, `x-dd-b`, …), if present.
    pub ray_id: Option<String>,
}

/// Case-insensitive header value lookup over a `(name, value)` slice.
fn hfind<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Detect a bot-management challenge in an HTTP response.
///
/// `headers` is a slice of `(name, value)` pairs (any case); `body` is the
/// response body (only the first ~256 KB are inspected to bound cost).
///
/// Returns `None` for ordinary responses, including normal pages served from
/// behind Cloudflare (a 200 with no challenge markers is not a challenge).
pub fn detect(status: u16, headers: &[(String, String)], body: &str) -> Option<DetectedChallenge> {
    // Inspect at most the leading 256 KiB — challenge markers are in the head.
    let scan = if body.len() > 256 * 1024 {
        &body[..256 * 1024]
    } else {
        body
    };
    let body_lc = scan.to_ascii_lowercase();
    let server = hfind(headers, "server").unwrap_or("");
    let set_cookie = hfind(headers, "set-cookie").unwrap_or("");
    let cf_mitigated = hfind(headers, "cf-mitigated");
    let cf_challenge_header = cf_mitigated
        .map(|s| s.eq_ignore_ascii_case("challenge"))
        .unwrap_or(false);

    // ── Cloudflare ────────────────────────────────────────────────────────
    let cf_served = server.eq_ignore_ascii_case("cloudflare");
    let cf_managed_body = body_lc.contains("just a moment")
        || body_lc.contains("checking your browser")
        || body_lc.contains("cdn-cgi/challenge-platform")
        || body_lc.contains("__cf_chl_")
        || body_lc.contains("cf-challenge");
    let cf_interactive = body_lc.contains("cf-turnstile")
        || body_lc.contains("turnstile")
        || body_lc.contains("hcaptcha")
        || body_lc.contains("g-recaptcha");
    let is_cf = cf_served || cf_mitigated.is_some() || cf_managed_body || cf_interactive;
    // Only report a real challenge, never a normal CF-served page.
    let cf_signal = cf_managed_body
        || cf_interactive
        || cf_challenge_header
        || cf_mitigated.is_some()
        || status == 503
        || (status == 403
            && (body_lc.contains("cloudflare") || body_lc.contains("attention required")));
    if is_cf && cf_signal {
        let kind = if cf_interactive {
            ChallengeKind::Interactive
        } else if cf_managed_body || cf_challenge_header {
            ChallengeKind::Managed
        } else if status == 403 {
            ChallengeKind::Blocked
        } else {
            ChallengeKind::Unknown
        };
        return Some(DetectedChallenge {
            vendor: ChallengeVendor::Cloudflare,
            kind,
            clearance_cookie: "cf_clearance",
            ray_id: hfind(headers, "cf-ray").map(String::from),
        });
    }

    // ── DataDome ──────────────────────────────────────────────────────────
    let dd_cookie = set_cookie.to_ascii_lowercase().contains("datadome=");
    let is_dd = server.eq_ignore_ascii_case("datadome")
        || dd_cookie
        || body_lc.contains("datadome")
        || body_lc.contains("cdn-dd")
        || body_lc.contains("dd-section");
    if is_dd
        && (status == 401 || status == 403 || status == 429 || body_lc.contains("interstitial"))
    {
        let kind = if body_lc.contains("captcha")
            || body_lc.contains("grecaptcha")
            || body_lc.contains("hcaptcha")
        {
            ChallengeKind::Interactive
        } else {
            ChallengeKind::Managed
        };
        return Some(DetectedChallenge {
            vendor: ChallengeVendor::DataDome,
            kind,
            clearance_cookie: "datadome",
            ray_id: hfind(headers, "x-dd-b")
                .or_else(|| hfind(headers, "x-ddb"))
                .map(String::from),
        });
    }

    // ── PerimeterX / HUMAN ────────────────────────────────────────────────
    let is_px = body_lc.contains("_pxcaptcha")
        || body_lc.contains("/px.gif")
        || body_lc.contains("px-cdn")
        || body_lc.contains("perimeterx")
        || hfind(headers, "x-px_blocks").is_some()
        || set_cookie.to_ascii_lowercase().contains("_px");
    if is_px && (status == 403 || status == 429 || status == 503) {
        let kind = if body_lc.contains("captcha") {
            ChallengeKind::Interactive
        } else {
            ChallengeKind::Managed
        };
        return Some(DetectedChallenge {
            vendor: ChallengeVendor::PerimeterX,
            kind,
            clearance_cookie: "_px3",
            ray_id: hfind(headers, "x-px").map(String::from),
        });
    }

    // ── Akamai / Imperva (Incapsula) ──────────────────────────────────────
    let set_cookie_lc = set_cookie.to_ascii_lowercase();
    let is_imperva = set_cookie_lc.contains("incap_ses")
        || set_cookie_lc.contains("visid_incap")
        || body_lc.contains("incapsula")
        || body_lc.contains("request unsuccessful");
    let is_akamai =
        server.eq_ignore_ascii_case("akamai") || server.eq_ignore_ascii_case("akamaighost");
    let akamai_block = body_lc.contains("access denied") && body_lc.contains("reference #");
    if (is_imperva || (is_akamai && akamai_block))
        && (status == 403 || status == 429 || status == 503)
    {
        return Some(DetectedChallenge {
            vendor: ChallengeVendor::AkamaiImperva,
            kind: ChallengeKind::Managed,
            clearance_cookie: if is_imperva { "incap_ses" } else { "ak_bmsc" },
            ray_id: None,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn cloudflare_managed_interstitial() {
        let h = hdrs(&[("server", "cloudflare"), ("cf-ray", "88abc-XYZ")]);
        let body = "<title>Just a moment...</title><script src=\"/cdn-cgi/challenge-platform/h/g/cv/result\"></script>";
        let d = detect(403, &h, body).expect("cf managed");
        assert_eq!(d.vendor, ChallengeVendor::Cloudflare);
        assert_eq!(d.kind, ChallengeKind::Managed);
        assert_eq!(d.clearance_cookie, "cf_clearance");
        assert_eq!(d.ray_id.as_deref(), Some("88abc-XYZ"));
    }

    #[test]
    fn cloudflare_cf_mitigated_header() {
        // No body markers, but cf-mitigated: challenge header signals it.
        let h = hdrs(&[("server", "cloudflare"), ("cf-mitigated", "challenge")]);
        let d = detect(403, &h, "<html>ok</html>").expect("cf via header");
        assert_eq!(d.vendor, ChallengeVendor::Cloudflare);
        assert_eq!(d.kind, ChallengeKind::Managed);
    }

    #[test]
    fn cloudflare_turnstile_is_interactive() {
        let h = hdrs(&[("server", "cloudflare")]);
        let body = "<div class=\"cf-turnstile\" data-sitekey=\"0xabc\"></div>";
        let d = detect(403, &h, body).expect("cf interactive");
        assert_eq!(d.kind, ChallengeKind::Interactive);
    }

    #[test]
    fn cloudflare_200_normal_page_not_a_challenge() {
        let h = hdrs(&[("server", "cloudflare"), ("cf-ray", "1")]);
        assert!(detect(200, &h, "<html><body>hello world</body></html>").is_none());
    }

    #[test]
    fn cloudflare_hard_403_is_blocked() {
        let h = hdrs(&[("server", "cloudflare")]);
        // 403 with "cloudflare" but no managed/interactive markers → Blocked.
        let d = detect(403, &h, "<title>cloudflare</title>access denied").expect("cf blocked");
        assert_eq!(d.kind, ChallengeKind::Blocked);
    }

    #[test]
    fn datadome_managed() {
        let h = hdrs(&[("server", "datadome"), ("x-dd-b", "DD-REQ-ID-1")]);
        let body = "<html data-dd=\"1\"><script src=\"https://cdn-dd.cs.net/c\"></script>interstitial</html>";
        let d = detect(403, &h, body).expect("datadome");
        assert_eq!(d.vendor, ChallengeVendor::DataDome);
        assert_eq!(d.clearance_cookie, "datadome");
        assert_eq!(d.ray_id.as_deref(), Some("DD-REQ-ID-1"));
    }

    #[test]
    fn datadome_via_set_cookie() {
        let h = hdrs(&[("set-cookie", "datadome=abc; path=/; samesite=lax")]);
        let d = detect(403, &h, "<html>blocked</html>").expect("datadome cookie");
        assert_eq!(d.vendor, ChallengeVendor::DataDome);
    }

    #[test]
    fn perimeterx_managed() {
        let h = hdrs(&[("x-px_blocks", "block")]);
        let body =
            "<script src=\"https://client.perimeterx.net/px-captcha.js\"></script>_pxCaptcha";
        let d = detect(403, &h, body).expect("px");
        assert_eq!(d.vendor, ChallengeVendor::PerimeterX);
        assert_eq!(d.clearance_cookie, "_px3");
    }

    #[test]
    fn imperva_via_incap_cookie() {
        let h = hdrs(&[("set-cookie", "incap_ses_123_45=xyz; path=/")]);
        let body = "<title>Request unsuccessful.</title>Incapsula";
        let d = detect(403, &h, body).expect("imperva");
        assert_eq!(d.vendor, ChallengeVendor::AkamaiImperva);
        assert_eq!(d.clearance_cookie, "incap_ses");
    }

    #[test]
    fn akamai_access_denied() {
        let h = hdrs(&[("server", "AkamaiGHost")]);
        let body = "<h1>Access Denied</h1><p>Reference #18.abc</p>";
        let d = detect(403, &h, body).expect("akamai");
        assert_eq!(d.vendor, ChallengeVendor::AkamaiImperva);
        assert_eq!(d.clearance_cookie, "ak_bmsc");
    }

    #[test]
    fn clean_responses_are_not_challenges() {
        let h = hdrs(&[("server", "nginx")]);
        assert!(detect(200, &h, "<html>ok</html>").is_none());
        assert!(detect(404, &h, "<html>not found</html>").is_none());
        assert!(detect(500, &h, "server error").is_none());
    }

    #[test]
    fn body_scan_is_bounded() {
        // A 1 MiB body with the marker only in the head is still detected.
        let head = "Just a moment...";
        let tail = "x".repeat(1024 * 1024);
        let body = format!("{head}{tail}");
        let h = hdrs(&[("server", "cloudflare")]);
        let d = detect(403, &h, &body).expect("bounded scan detects head marker");
        assert_eq!(d.vendor, ChallengeVendor::Cloudflare);
    }
}
