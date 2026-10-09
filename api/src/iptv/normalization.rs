//! Channel name normalization — the heart of the matching pipeline.
//!
//! Strips provider noise, extracts country/quality metadata, and produces a
//! clean canonical name suitable for fuzzy matching and deduplication.

use regex::Regex;
use std::sync::LazyLock;

/// Result of normalizing a raw IPTV stream name.
pub struct NormalizedStream {
    pub normalized_name: String,
    /// Quality tier: "4k", "fhd", "hd", "sd", or "unknown".
    pub quality: String,
    /// ISO 3166-1 alpha-2 country code (e.g. "GB", "US").
    pub country: Option<String>,
    /// ISO 639-1 language code (e.g. "en", "fr").
    pub language: Option<String>,
}

// -- Compiled regexes --------------------------------------------------------

static EMOJI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\x{2500}-\x{25FF}\x{2190}-\x{21FF}\x{2600}-\x{26FF}\x{2700}-\x{27BF}\x{FE00}-\x{FE0F}\x{1F000}-\x{1FAFF}\x{200D}\x{20E3}\x{E0020}-\x{E007F}]",
    )
    .expect("emoji regex")
});

/// Only strip square brackets (provider tags like [VIP], [HD]).
/// Parentheses are handled selectively by NOISE_PAREN_RE.
static SQUARE_BRACKET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[.*?\]").expect("square bracket regex"));

/// Strip parenthesized content only if it contains noise: quality markers,
/// provider tags, dates, or is entirely numeric.
static NOISE_PAREN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\(\s*(?:HD|FHD|4K|UHD|SD|720P|1080P|480P|FULL\s?HD|BACKUP|BK|ALT|VIP|PREMIUM|HEVC|H\.?265|H\.?264|MPEG[24]?|AAC|AC3|MULTI|NEW|\d{4}-\d{2}-\d{2}[\s\d:]*)\s*\)")
        .expect("noise paren regex")
});

static QUALITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(4K|UHD|3840P|2160P|FHD|FULL\s?HD|1080P|HD|720P|SD|480P|LQ)\b")
        .expect("quality regex")
});

/// Country/provider prefix regex — matches at start of string before a `|`, `:`, or `-` separator.
/// Includes standard ISO codes plus common provider-specific prefixes (MXC, BEE, EN, ALB, SLING, PRIME).
static COUNTRY_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Country codes (2-letter ISO + common 3-letter variants) and provider names.
    // Provider-only prefixes (SLING, PRIME, OD, GO, VIX, JOYN, CITY, YP, MG, MO, EXYU, CRB)
    // are stripped from the name but don't set a country code.
    // Longer alternatives MUST come before shorter ones to prevent partial matches
    // (e.g. "NOR" must precede "NO", "ALB" must precede "AL", "SWE" must precede "SE").
    Regex::new(r"(?i)^(UNITED\s+KINGDOM|ENGLAND|SCOTLAND|WALES|IRELAND|HINDI|TAMIL|SLING|PRIME|JOYN|CITY|EXYU|ASIA|PLAY\+|NOW|MXC|SWE|NOR|ALB|BEE|VIX|LAT|AFR|ISR|CRB|DNK|ARG|FIN|BAN|AUS|UK|US|FR|DE|ES|IT|PT|NL|BE|AR|MX|BR|CA|AU|IN|PK|TR|PL|RO|GR|SE|NO|DK|FI|CZ|HU|BG|HR|RS|UA|RU|JP|KR|CN|TW|TH|PH|ID|MY|SG|ZA|EG|SA|AE|IL|AT|CH|IS|IR|NZ|AL|MK|CY|BH|NA|AM|AZ|UZ|TJ|BA|VN|HK|EN|OD|GO|YP|MG|MO|KU|SL|CG|EU|V\+)\s*[:|\-]\s*")
        .expect("country prefix regex")
});

static NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(VIP|PREMIUM|BACKUP|BK|ALT|MULTI|NEW|HEVC|H\.?265|H\.?264|MPEG[24]?|AAC|AC3|RAW|GOLD)\b")
        .expect("noise regex")
});

static FRAMERATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b\d+\s?FPS\b").expect("framerate regex"));

static SEPARATOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*[|]\s*|\s*:\s*|\s+- ").expect("separator regex"));

static MULTI_SPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s{2,}").expect("multi-space regex"));

/// Normalize "sport" followed by a digit to "sports" — groups "TNT Sport 2" with "TNT Sports 2".
/// Only applies before numbers to avoid mangling brand names like "Sky Sport Bundesliga".
static SPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bsport\s+(\d)").expect("sport regex"));

/// Category prefix regex — extract the part before `|` in category names like "UK| GENERAL".
static CATEGORY_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(HINDI|SLING|PRIME|EXYU|ASIA|MXC|SWE|NOR|ALB|BEE|LAT|AFR|ISR|UK|US|FR|DE|ES|IT|PT|NL|BE|AR|MX|BR|CA|AU|IN|PK|TR|PL|RO|GR|SE|NO|DK|FI|CZ|HU|BG|HR|RS|UA|RU|JP|KR|CN|TW|TH|PH|ID|MY|SG|ZA|EG|SA|AE|IL|AT|CH|IS|IR|NZ|AL|MK|CY|BH|EN|KU|SL|CG|EU|NA)\s*\|")
        .expect("category prefix regex")
});

/// Convert Unicode superscript letters/digits to their ASCII equivalents.
///
/// IPTV providers use superscripts like `ᴴᴰ` for "HD", `ᶠᴴᴰ` for "FHD".
/// Must run BEFORE lowercasing so the quality regex can match them.
fn convert_superscripts(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{1D2C}' => 'A',
            '\u{1D2E}' => 'B',
            '\u{1D9C}' => 'C',
            '\u{1D30}' => 'D',
            '\u{1D31}' => 'E',
            '\u{1DA0}' => 'F',
            '\u{1D33}' => 'G',
            '\u{1D34}' => 'H',
            '\u{1D35}' => 'I',
            '\u{1D36}' => 'J',
            '\u{1D37}' => 'K',
            '\u{1D38}' => 'L',
            '\u{1D39}' => 'M',
            '\u{1D3A}' => 'N',
            '\u{1D3C}' => 'O',
            '\u{1D3E}' => 'P',
            '\u{1D3F}' => 'R',
            '\u{02E2}' => 'S',
            '\u{1D40}' => 'T',
            '\u{1D41}' => 'U',
            '\u{2C7D}' => 'V',
            '\u{1D42}' => 'W',
            '\u{2070}' => '0',
            '\u{00B9}' => '1',
            '\u{00B2}' => '2',
            '\u{00B3}' => '3',
            '\u{2074}' => '4',
            '\u{2075}' => '5',
            '\u{2076}' => '6',
            '\u{2077}' => '7',
            '\u{2078}' => '8',
            '\u{2079}' => '9',
            // Modifier letter (lowercase) superscripts — used by some providers
            // for codec tags like ʰᵉᵛᶜ (HEVC) or quality like ⁴ᵏ (4K).
            '\u{02B0}' => 'h', // MODIFIER LETTER SMALL H
            '\u{1D49}' => 'e', // MODIFIER LETTER SMALL E
            '\u{1D5B}' => 'v', // MODIFIER LETTER SMALL V
            '\u{1D4F}' => 'k', // MODIFIER LETTER SMALL K
            '\u{1D50}' => 'm', // MODIFIER LETTER SMALL M
            '\u{207F}' => 'n', // SUPERSCRIPT LATIN SMALL LETTER N
            '\u{1D56}' => 'p', // MODIFIER LETTER SMALL P (distinct from U+1D3E capital P)
            _ => c,
        })
        .collect()
}

/// Normalize a raw IPTV stream name into a canonical form with extracted metadata.
pub fn normalize_stream_name(raw_name: &str) -> NormalizedStream {
    // Convert Unicode superscripts first (before lowercasing)
    let desuperscripted = convert_superscripts(raw_name);
    let mut name = desuperscripted.to_lowercase();

    // Strip Unicode decorations (emojis, box-drawing, arrows, etc.)
    name = EMOJI_RE.replace_all(&name, "").into_owned();

    // Strip # decorations (e.g. "##### ENTERTAINMENT #####")
    name = name.replace('#', " ");

    // Extract country prefix BEFORE converting + to "plus" (PLAY+| is a provider prefix)
    let country = extract_country_prefix(&name);
    if COUNTRY_PREFIX_RE.is_match(&name) {
        name = COUNTRY_PREFIX_RE.replace(&name, "").into_owned();
    }

    // Convert '+' to " plus " for brand names like ESPN+, Canal+ (AFTER prefix extraction)
    name = name.replace('+', " plus ");

    // Extract quality (anywhere in string)
    let quality = extract_quality(&name);
    name = QUALITY_RE.replace_all(&name, "").into_owned();

    // Strip square brackets entirely (provider tags like [VIP], [HD])
    name = SQUARE_BRACKET_RE.replace_all(&name, "").into_owned();

    // Strip only noise-containing parentheses (dates, quality, provider tags)
    name = NOISE_PAREN_RE.replace_all(&name, "").into_owned();

    // Strip provider noise words
    name = NOISE_RE.replace_all(&name, "").into_owned();

    // Strip frame rate indicators (e.g. "50 FPS", "60fps")
    name = FRAMERATE_RE.replace_all(&name, "").into_owned();

    // Normalize common singular/plural variations (only before digits)
    name = SPORT_RE.replace_all(&name, "sports $1").into_owned();

    // Strip empty parentheses left behind by quality/noise stripping
    name = name.replace("()", "").replace("( )", "");

    // Strip separators (but not hyphens within words)
    name = SEPARATOR_RE.replace_all(&name, " ").into_owned();

    // Collapse whitespace, trim
    name = MULTI_SPACE_RE.replace_all(&name, " ").into_owned();
    name = name.trim().to_string();

    // Fallback: if normalized name is too short (<=2 chars), it was probably
    // over-stripped. Use the original name lowercased and trimmed instead.
    if name.len() <= 2 {
        let fallback = raw_name.to_lowercase().trim().to_string();
        // Re-extract just the country prefix to strip it from the fallback
        let fb_stripped = COUNTRY_PREFIX_RE.replace(&fallback, "").into_owned();
        let fb_stripped = fb_stripped.trim().to_string();
        if fb_stripped.len() > 2 {
            name = fb_stripped;
        } else if fallback.len() > 2 {
            name = fallback;
        }
    }

    // Derive language from country
    let language = country
        .as_deref()
        .and_then(country_to_language)
        .map(String::from);

    NormalizedStream {
        normalized_name: name,
        quality,
        country,
        language,
    }
}

/// Extract and map a country prefix to an ISO 3166-1 alpha-2 code.
fn extract_country_prefix(lowered: &str) -> Option<String> {
    let caps = COUNTRY_PREFIX_RE.captures(lowered)?;
    let raw = caps.get(1)?.as_str().trim();
    let code = normalize_country_code(raw);
    // Return None for provider-only prefixes (not real countries)
    if code == "XX" {
        return None;
    }
    Some(code.to_string())
}

/// Map raw country string to ISO 3166-1 alpha-2.
pub fn normalize_country_code(raw: &str) -> &'static str {
    match raw.to_uppercase().as_str() {
        "UK" | "UNITED KINGDOM" | "ENGLAND" | "SCOTLAND" | "WALES" => "GB",
        "IRELAND" => "IE",
        "US" => "US",
        "CA" => "CA",
        "AU" | "NZ" => "AU", // NZ grouped with AU for practical purposes
        "FR" => "FR",
        "DE" | "AT" => "DE", // Austria uses German-language content
        "ES" => "ES",
        "IT" => "IT",
        "PT" => "PT",
        "NL" => "NL",
        "BE" => "BE",
        "AR" | "ARG" => "AR",
        "MX" | "MXC" => "MX",
        "BR" => "BR",
        "IN" | "HINDI" | "TAMIL" => "IN",
        "PK" => "PK",
        "TR" => "TR",
        "PL" => "PL",
        "RO" => "RO",
        "GR" | "CY" => "GR",
        "SE" | "SWE" => "SE",
        "NO" | "NOR" => "NO",
        "DK" | "DNK" => "DK",
        "FI" | "FIN" => "FI",
        "CZ" => "CZ",
        "HU" => "HU",
        "BG" => "BG",
        "HR" => "HR",
        "RS" | "EXYU" => "RS", // ex-Yugoslavia content → Serbia
        "UA" => "UA",
        "RU" => "RU",
        "JP" => "JP",
        "KR" => "KR",
        "CN" => "CN",
        "TW" => "TW",
        "TH" => "TH",
        "PH" => "PH",
        "ID" => "ID",
        "MY" => "MY",
        "SG" => "SG",
        "ZA" | "AFR" => "ZA", // AFR (Africa) → South Africa as best guess
        "EG" => "EG",
        "SA" => "SA",
        "AE" => "AE",
        "IL" | "ISR" => "IL",
        "IR" => "IR",
        "IS" => "IS",
        "CH" => "CH",
        "MK" => "MK",
        "BH" => "BH",
        "AM" => "AM",
        "AZ" => "AZ",
        "UZ" => "UZ",
        "TJ" => "TJ",
        // EN maps to GB (English-language content)
        "EN" => "GB",
        // ALB maps to Albania
        "AL" | "ALB" => "AL",
        // KU = Kurdish (no specific country, use Iraq as proxy)
        "KU" => "IQ",
        // LAT = Latin America (use MX as proxy)
        "LAT" => "MX",
        // ASIA = generic Asian content
        "ASIA" => "XX",
        "BA" => "BA",  // Bosnia
        "VN" => "VN",  // Vietnam
        "HK" => "HK",  // Hong Kong
        "AUS" => "AU", // Australia (3-letter variant, "AU" already matched above)
        "BAN" => "BD", // Bangladesh
        // Provider names — not countries, just strip them
        "SLING" | "PRIME" | "OD" | "GO" | "VIX" | "JOYN" | "CITY" | "YP" | "MG" | "MO" | "CRB"
        | "SL" | "CG" | "EU" | "NA" | "PLAY+" | "V+" | "BEE" | "NOW" => "XX",
        _ => "XX",
    }
}

/// Extract quality from the lowercased name.
fn extract_quality(lowered: &str) -> String {
    let Some(caps) = QUALITY_RE.captures(lowered) else {
        return "unknown".to_string();
    };
    let raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    match raw.to_uppercase().as_str() {
        "4K" | "UHD" | "3840P" | "2160P" => "4k",
        "FHD" | "FULLHD" | "FULL HD" => "fhd",
        "1080P" => "fhd",
        "HD" | "720P" => "hd",
        "SD" | "480P" | "LQ" => "sd",
        _ => "unknown",
    }
    .to_string()
}

/// Detect language from provider category name.
///
/// First tries to extract a country prefix before `|` (e.g., "UK| GENERAL" → "en"),
/// then falls back to keyword matching in the full category name.
pub fn detect_language_from_category(category_name: &str) -> Option<&'static str> {
    // Try prefix extraction first: "UK| GENERAL" → extract "UK" → "en"
    if let Some(caps) = CATEGORY_PREFIX_RE.captures(category_name) {
        let prefix = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let country = normalize_country_code(prefix);
        if country != "XX"
            && let Some(lang) = country_to_language(country)
        {
            return Some(lang);
        }
    }

    // Fall back to keyword matching
    let lower = category_name.to_lowercase();
    let mappings: &[(&[&str], &str)] = &[
        (
            &[
                "english",
                "usa",
                "united states",
                "united kingdom",
                "british",
                "australian",
            ],
            "en",
        ),
        (&["french", "france", "francais"], "fr"),
        (&["arabic", "arab"], "ar"),
        (&["spanish", "spain", "espanol", "latino", "latina"], "es"),
        (&["german", "germany", "deutsch"], "de"),
        (&["italian", "italy", "italiano"], "it"),
        (&["portuguese", "portugal", "brazil", "brasil"], "pt"),
        (&["turkish", "turkey", "turk"], "tr"),
        (&["russian", "russia"], "ru"),
        (&["hindi", "india", "indian", "bollywood"], "hi"),
        (&["urdu", "pakistan"], "ur"),
        (&["dutch", "netherlands", "holland"], "nl"),
        (&["polish", "poland", "polska"], "pl"),
        (&["romanian", "romania"], "ro"),
        (&["greek", "greece"], "el"),
        (&["swedish", "sweden"], "sv"),
        (&["norwegian", "norway"], "no"),
        (&["danish", "denmark"], "da"),
        (&["finnish", "finland"], "fi"),
        (&["czech", "czechia"], "cs"),
        (&["hungarian", "hungary"], "hu"),
        (&["korean", "korea"], "ko"),
        (&["japanese", "japan"], "ja"),
        (&["chinese", "china", "mandarin", "cantonese"], "zh"),
        (&["thai", "thailand"], "th"),
        (&["filipino", "philippines", "pinoy"], "tl"),
        (&["malay", "malaysia"], "ms"),
        (&["persian", "farsi", "iran"], "fa"),
        (&["albanian", "albania"], "sq"),
        (&["serbian", "serbia"], "sr"),
        (&["croatian", "croatia"], "hr"),
        (&["bulgarian", "bulgaria"], "bg"),
        (&["ukrainian", "ukraine"], "uk"),
    ];
    for (keywords, lang) in mappings {
        for kw in *keywords {
            if lower.contains(kw) {
                return Some(lang);
            }
        }
    }
    None
}

/// Map ISO country code to ISO 639-1 language code.
pub fn country_to_language(country: &str) -> Option<&'static str> {
    match country {
        "GB" | "US" | "AU" | "CA" | "IE" | "NZ" => Some("en"),
        "FR" | "BE" => Some("fr"),
        "DE" | "AT" | "CH" => Some("de"),
        "ES" | "MX" | "AR" => Some("es"),
        "IT" => Some("it"),
        "PT" | "BR" => Some("pt"),
        "NL" => Some("nl"),
        "TR" => Some("tr"),
        "PL" => Some("pl"),
        "RO" => Some("ro"),
        "GR" | "CY" => Some("el"),
        "SE" => Some("sv"),
        "NO" => Some("no"),
        "DK" => Some("da"),
        "FI" => Some("fi"),
        "CZ" => Some("cs"),
        "HU" => Some("hu"),
        "BG" => Some("bg"),
        "HR" => Some("hr"),
        "RS" | "MK" => Some("sr"),
        "UA" => Some("uk"),
        "RU" => Some("ru"),
        "JP" => Some("ja"),
        "KR" => Some("ko"),
        "CN" | "TW" | "HK" => Some("zh"),
        "TH" => Some("th"),
        "VN" => Some("vi"),
        "PH" => Some("tl"), // Tagalog
        "IN" => Some("hi"), // Hindi (dominant IPTV language)
        "PK" => Some("ur"), // Urdu
        "BD" => Some("bn"), // Bengali
        "SG" => Some("en"), // English dominant
        "ID" | "MY" => Some("ms"),
        "EG" | "SA" | "AE" | "BH" => Some("ar"),
        "IL" => Some("he"),
        "IR" => Some("fa"),
        "IS" => Some("is"),
        "AL" => Some("sq"),
        "IQ" => Some("ku"),
        "AM" => Some("hy"),
        "AZ" => Some("az"),
        "UZ" => Some("uz"),
        "TJ" => Some("tg"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_normalization() {
        let result = normalize_stream_name("UK: BBC One HD");
        assert_eq!(result.normalized_name, "bbc one");
        assert_eq!(result.quality, "hd");
        assert_eq!(result.country.as_deref(), Some("GB"));
        assert_eq!(result.language.as_deref(), Some("en"));
    }

    #[test]
    fn country_prefix_dash_separator() {
        // B1G/skyline-mm style: "UK-Channel Name" without a colon or pipe.
        let result = normalize_stream_name("UK-TNT Sports 2 FHD HEVC");
        assert_eq!(result.normalized_name, "tnt sports 2");
        assert_eq!(result.country.as_deref(), Some("GB"));
    }

    #[test]
    fn strips_quality_suffixes() {
        let result = normalize_stream_name("Sky Sports 4K UHD");
        assert_eq!(result.quality, "4k");
        assert!(!result.normalized_name.contains("4k"));
    }

    #[test]
    fn strips_noise_and_brackets() {
        let result = normalize_stream_name("US: CNN [VIP] (BACKUP)");
        assert_eq!(result.normalized_name, "cnn");
        assert_eq!(result.country.as_deref(), Some("US"));
    }

    #[test]
    fn handles_fhd_variants() {
        assert_eq!(normalize_stream_name("Channel FHD").quality, "fhd");
        assert_eq!(normalize_stream_name("Channel FULLHD").quality, "fhd");
        assert_eq!(normalize_stream_name("Channel 1080P").quality, "fhd");
    }

    #[test]
    fn no_country_prefix() {
        let result = normalize_stream_name("Discovery Channel HD");
        assert!(result.country.is_none());
        assert_eq!(result.quality, "hd");
        assert_eq!(result.normalized_name, "discovery channel");
    }

    #[test]
    fn england_maps_to_gb() {
        let result = normalize_stream_name("ENGLAND: ITV1");
        assert_eq!(result.country.as_deref(), Some("GB"));
        assert_eq!(result.language.as_deref(), Some("en"));
    }

    #[test]
    fn pipe_separator_stripped() {
        let result = normalize_stream_name("FR: TF1 | FHD");
        assert_eq!(result.normalized_name, "tf1");
        assert_eq!(result.quality, "fhd");
        assert_eq!(result.country.as_deref(), Some("FR"));
    }

    #[test]
    fn spanish_country_mapping() {
        for prefix in ["ES", "MX", "AR"] {
            let result = normalize_stream_name(&format!("{prefix}: TestChannel"));
            assert_eq!(
                result.language.as_deref(),
                Some("es"),
                "failed for {prefix}"
            );
        }
    }

    #[test]
    fn keeps_parenthetical_channel_names() {
        // "US (Paramount 047) | (2098-12-31 09:00:01)" should keep "paramount 047"
        let result = normalize_stream_name("US| (Paramount 047) | (2098-12-31 09:00:01)");
        assert!(
            result.normalized_name.contains("paramount 047"),
            "got: '{}'",
            result.normalized_name
        );
        assert_eq!(result.country.as_deref(), Some("US"));
    }

    #[test]
    fn mxc_prefix_maps_to_mx() {
        let result = normalize_stream_name("MXC| AZTECA 7");
        assert_eq!(result.country.as_deref(), Some("MX"));
        assert_eq!(result.language.as_deref(), Some("es"));
        assert_eq!(result.normalized_name, "azteca 7");
    }

    #[test]
    fn en_prefix_maps_to_gb() {
        let result = normalize_stream_name("EN| BBC News");
        assert_eq!(result.country.as_deref(), Some("GB"));
        assert_eq!(result.language.as_deref(), Some("en"));
    }

    #[test]
    fn alb_prefix_maps_to_al() {
        let result = normalize_stream_name("ALB| Top Channel");
        assert_eq!(result.country.as_deref(), Some("AL"));
        assert_eq!(result.language.as_deref(), Some("sq"));
    }

    #[test]
    fn sling_prime_are_stripped_not_country() {
        let result = normalize_stream_name("SLING| ESPN");
        assert!(result.country.is_none());
        assert_eq!(result.normalized_name, "espn");

        let result = normalize_stream_name("PRIME| National Geographic");
        assert!(result.country.is_none());
        assert_eq!(result.normalized_name, "national geographic");
    }

    #[test]
    fn numbers_preserved_in_channel_names() {
        let result = normalize_stream_name("JKN 18");
        assert_eq!(result.normalized_name, "jkn 18");
    }

    #[test]
    fn short_name_fallback() {
        // If stripping produces <=2 chars, use fallback
        let result = normalize_stream_name("US| HD");
        // "HD" is a quality marker that gets stripped → empty → fallback
        assert!(
            result.normalized_name.len() > 2,
            "got: '{}'",
            result.normalized_name
        );
    }

    #[test]
    fn nor_prefix_maps_to_no() {
        let result = normalize_stream_name("NOR| TV2 SPORT 2 HD");
        assert_eq!(result.country.as_deref(), Some("NO"));
        assert_eq!(result.language.as_deref(), Some("no"));
        assert_eq!(result.normalized_name, "tv2 sports 2");
    }

    #[test]
    fn swe_prefix_maps_to_se() {
        let result = normalize_stream_name("SWE| SVT1");
        assert_eq!(result.country.as_deref(), Some("SE"));
        assert_eq!(result.normalized_name, "svt1");
    }

    #[test]
    fn category_language_from_prefix() {
        assert_eq!(detect_language_from_category("UK| GENERAL"), Some("en"));
        assert_eq!(detect_language_from_category("FR| CINEMA"), Some("fr"));
        assert_eq!(
            detect_language_from_category("DE| GERMANY HEVC"),
            Some("de")
        );
        assert_eq!(detect_language_from_category("AR| BEIN SPORT"), Some("es")); // AR = Argentina
        assert_eq!(detect_language_from_category("MXC| IPTV"), Some("es"));
        assert_eq!(detect_language_from_category("ALB| CHANNELS"), Some("sq"));
        assert_eq!(detect_language_from_category("BEE| DAZN PPV"), None); // BEE is a provider prefix, not Belgium
        assert_eq!(
            detect_language_from_category("BE| ENTERTAINMENT"),
            Some("fr")
        ); // BE → Belgium → fr
        assert_eq!(detect_language_from_category("CA| ENGLISH"), Some("en"));
    }

    #[test]
    fn category_language_keyword_fallback() {
        assert_eq!(detect_language_from_category("24/7 MOVIES & SERIES"), None);
        assert_eq!(
            detect_language_from_category("- NO EVENT STREAMING -"),
            None
        );
    }

    #[test]
    fn greek_language_code_is_el() {
        // GR → "el" (ISO 639-1 for Greek), not "gr"
        assert_eq!(country_to_language("GR"), Some("el"));
    }

    #[test]
    fn unicode_superscript_hd() {
        // ᴴᴰ (U+1D34 U+1D30) should be converted to HD and stripped as quality
        let result = normalize_stream_name("UK| BBC ONE \u{1D34}\u{1D30}");
        assert_eq!(result.normalized_name, "bbc one");
        assert_eq!(result.quality, "hd");
        assert_eq!(result.country.as_deref(), Some("GB"));
    }

    #[test]
    fn unicode_superscript_fhd() {
        // ᶠᴴᴰ (U+1DA0 U+1D34 U+1D30) should be converted to FHD
        let result = normalize_stream_name("US| ESPN \u{1DA0}\u{1D34}\u{1D30}");
        assert_eq!(result.quality, "fhd");
        assert_eq!(result.normalized_name, "espn");
    }

    #[test]
    fn plus_converted_to_word() {
        // ESPN+ → "espn plus", Canal+ → "canal plus"
        let result = normalize_stream_name("US| ESPN+ FHD");
        assert_eq!(result.normalized_name, "espn plus");
        assert_eq!(result.quality, "fhd");

        let result = normalize_stream_name("FR| Canal+ 4K");
        assert_eq!(result.normalized_name, "canal plus");
        assert_eq!(result.quality, "4k");
    }

    #[test]
    fn ampersand_preserved() {
        // A&E is a brand name — & should survive normalization
        let result = normalize_stream_name("A&E HD");
        assert_eq!(result.normalized_name, "a&e");
        assert_eq!(result.quality, "hd");
    }

    #[test]
    fn tnt_sports_number_preserved() {
        let result = normalize_stream_name("UK| TNT Sports 2 HD");
        assert_eq!(result.normalized_name, "tnt sports 2");
        assert_eq!(result.quality, "hd");
    }

    #[test]
    fn twenty_four_seven_preserved() {
        let result = normalize_stream_name("24/7 KISSING GAME");
        assert!(
            result.normalized_name.contains("24/7"),
            "got: '{}'",
            result.normalized_name
        );
    }

    /// Verify normalization against real-world IPTV stream names.
    /// These are actual stream names from a live provider.
    #[test]
    fn real_world_streams() {
        let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
            // (input, expected_normalized, expected_quality, expected_country)
            // BBC ONE: different quality tiers should all normalize to same name
            ("UK| BBC ONE HD", "bbc one", "hd", Some("GB")),
            ("UK| BBC ONE FHD", "bbc one", "fhd", Some("GB")),
            ("NL| BBC ONE 4K", "bbc one", "4k", Some("NL")),
            ("NL| BBC ONE HD", "bbc one", "hd", Some("NL")),
            // TNT Sports: numbers are part of the channel name
            ("UK| TNT SPORTS 1 HD", "tnt sports 1", "hd", Some("GB")),
            ("UK| TNT SPORTS 2 HD", "tnt sports 2", "hd", Some("GB")),
            ("UK| TNT SPORTS 1 FHD", "tnt sports 1", "fhd", Some("GB")),
            // ESPN: basic quality stripping
            ("US| ESPN HD", "espn", "hd", Some("US")),
            // Canal+: plus sign conversion
            ("FR| CANAL+ HD", "canal plus", "hd", Some("FR")),
            ("FR| CANAL+ FHD", "canal plus", "fhd", Some("FR")),
            // MXC prefix
            ("MXC| AZTECA 7", "azteca 7", "unknown", Some("MX")),
            ("MXC| CANAL 5", "canal 5", "unknown", Some("MX")),
            // 24/7 preserved
            ("24/7 KISSING GAME", "24/7 kissing game", "unknown", None),
            // Australian with city
            ("AU| 7TWO SYDNEY", "7two sydney", "unknown", Some("AU")),
            // German sports
            (
                "DE| SKY SPORT BUNDESLIGA HD",
                "sky sport bundesliga",
                "hd",
                Some("DE"),
            ),
            // Unicode superscripts (real provider data)
            ("UK| BBC TWO HD", "bbc two", "hd", Some("GB")),
            // Sky Sports: quality variants should group
            (
                "UK| SKY SPORTS MAIN EVENT FHD",
                "sky sports main event",
                "fhd",
                Some("GB"),
            ),
        ];

        let mut failures = Vec::new();
        for (input, exp_name, exp_quality, exp_country) in &cases {
            let result = normalize_stream_name(input);
            let mut issues = Vec::new();
            if result.normalized_name != *exp_name {
                issues.push(format!(
                    "name: got '{}' expected '{}'",
                    result.normalized_name, exp_name
                ));
            }
            if result.quality != *exp_quality {
                issues.push(format!(
                    "quality: got '{}' expected '{}'",
                    result.quality, exp_quality
                ));
            }
            if result.country.as_deref() != *exp_country {
                issues.push(format!(
                    "country: got {:?} expected {:?}",
                    result.country, exp_country
                ));
            }
            if !issues.is_empty() {
                failures.push(format!("\"{}\":\n  {}", input, issues.join("\n  ")));
            }
        }

        assert!(
            failures.is_empty(),
            "Normalization failures:\n{}",
            failures.join("\n")
        );
    }

    /// Verify that quality variants of the same channel produce the same normalized name.
    #[test]
    fn quality_variants_group_together() {
        let groups: Vec<Vec<&str>> = vec![
            vec!["UK| BBC ONE HD", "UK| BBC ONE FHD", "UK| BBC ONE HEVC"],
            vec![
                "UK| TNT SPORTS 1 HD",
                "UK| TNT SPORTS 1 FHD",
                "UK| TNT SPORTS 1 SD",
            ],
            vec!["FR| CANAL+ HD", "FR| CANAL+ FHD"],
            vec![
                "DE| SKY SPORT BUNDESLIGA HD",
                "DE| SKY SPORT BUNDESLIGA FHD",
            ],
        ];

        for group in &groups {
            let names: Vec<String> = group
                .iter()
                .map(|s| normalize_stream_name(s).normalized_name)
                .collect();
            let first = &names[0];
            for (i, name) in names.iter().enumerate().skip(1) {
                assert_eq!(
                    first, name,
                    "Quality variants should group:\n  '{}' -> '{}'\n  '{}' -> '{}'",
                    group[0], first, group[i], name
                );
            }
        }
    }

    /// Verify that different channels with same base name but different numbers stay separate.
    #[test]
    fn numbered_channels_stay_separate() {
        let n1 = normalize_stream_name("UK| TNT SPORTS 1 HD");
        let n2 = normalize_stream_name("UK| TNT SPORTS 2 HD");
        assert_ne!(
            n1.normalized_name, n2.normalized_name,
            "TNT Sports 1 and TNT Sports 2 should be different channels"
        );
    }

    #[test]
    fn geometric_shapes_stripped() {
        // ◉ (U+25C9), ● (U+25CF), ■ (U+25A0) should all be stripped
        let result = normalize_stream_name("UK: TNT SPORTS 2 \u{1D34}\u{1D30} \u{25C9}");
        assert_eq!(result.normalized_name, "tnt sports 2");

        let result = normalize_stream_name("UK| BBC ONE \u{25CF}");
        assert_eq!(result.normalized_name, "bbc one");

        let result = normalize_stream_name("UK| SKY NEWS \u{25A0}");
        assert_eq!(result.normalized_name, "sky news");
    }

    #[test]
    fn sport_singular_normalizes_to_sports() {
        let n1 = normalize_stream_name("UK| TNT SPORT 2 HD");
        let n2 = normalize_stream_name("UK| TNT SPORTS 2 HD");
        assert_eq!(n1.normalized_name, n2.normalized_name);
        assert_eq!(n1.normalized_name, "tnt sports 2");

        // Ensure "sports" stays "sports" (not "sportss")
        let n3 = normalize_stream_name("UK| SKY SPORTS NEWS");
        assert_eq!(n3.normalized_name, "sky sports news");

        // "sport" without a following number stays as-is (brand names)
        let n4 = normalize_stream_name("DE| SKY SPORT BUNDESLIGA HD");
        assert_eq!(n4.normalized_name, "sky sport bundesliga");

        let n5 = normalize_stream_name("SPORT TV");
        assert_eq!(n5.normalized_name, "sport tv");
    }

    #[test]
    fn framerate_stripped() {
        let result = normalize_stream_name(
            "VIP: TNT SPORTS 2 \u{1D3F}\u{1D2C}\u{1D42} \u{2075}\u{2070} FPS",
        );
        assert_eq!(result.normalized_name, "tnt sports 2");

        let result = normalize_stream_name("UK| BBC ONE 60FPS");
        assert_eq!(result.normalized_name, "bbc one");

        let result = normalize_stream_name("US| ESPN 25 fps");
        assert_eq!(result.normalized_name, "espn");
    }

    #[test]
    fn lowercase_superscripts_converted() {
        // ʰᵉᵛᶜ (modifier letter small h/e/v + modifier letter small c)
        // should convert to "hevc" and get stripped by NOISE_RE
        let result = normalize_stream_name(
            "VIP: TNT SPORT 2 \u{1D34}\u{1D30} \u{02B0}\u{1D49}\u{1D5B}\u{1D9C}",
        );
        assert_eq!(result.normalized_name, "tnt sports 2");

        // ⁴ᵏ should convert to "4k" and get stripped as quality
        let result = normalize_stream_name("VIP: TNT SPORTS ULTIMATE \u{2074}\u{1D4F}");
        assert_eq!(result.normalized_name, "tnt sports ultimate");
        assert_eq!(result.quality, "4k");
    }

    #[test]
    fn resolution_3840p_stripped() {
        // ³⁸⁴⁰ᴾ → "3840P" → stripped as 4k quality
        let result = normalize_stream_name(
            "UK: TNT SPORT ULTIMATE \u{1D41}\u{1D34}\u{1D30} \u{00B3}\u{2078}\u{2074}\u{2070}\u{1D3E}",
        );
        assert_eq!(result.quality, "4k");
        assert!(
            !result.normalized_name.contains("3840"),
            "got: '{}'",
            result.normalized_name
        );

        let result = normalize_stream_name("UK| BBC ONE 2160P");
        assert_eq!(result.quality, "4k");
        assert_eq!(result.normalized_name, "bbc one");
    }

    #[test]
    fn now_provider_prefix_stripped() {
        let result = normalize_stream_name("NOW: TNT SPORT 1 \u{1D3F}\u{1D2C}\u{1D42}");
        assert!(result.country.is_none());
        assert_eq!(result.normalized_name, "tnt sports 1");

        let result = normalize_stream_name("NOW: TNT SPORTS 4 \u{1D3F}\u{1D2C}\u{1D42}");
        assert!(result.country.is_none());
        assert_eq!(result.normalized_name, "tnt sports 4");
    }

    #[test]
    fn country_code_normalization() {
        assert_eq!(normalize_country_code("UK"), "GB");
        assert_eq!(normalize_country_code("UNITED KINGDOM"), "GB");
        assert_eq!(normalize_country_code("ENGLAND"), "GB");
        assert_eq!(normalize_country_code("NOW"), "XX");
        assert_eq!(normalize_country_code("VIP"), "XX");
    }

    #[test]
    fn real_world_new_provider_streams() {
        // Real streams from new provider causing duplication
        let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
            // "TNT Sport 2 ᴴᴰ ʰᵉᵛᶜ" with VIP prefix — hevc stripped, sport→sports
            (
                "VIP: TNT SPORT 2 \u{1D34}\u{1D30} \u{02B0}\u{1D49}\u{1D5B}\u{1D9C}",
                "tnt sports 2",
                "hd",
                None,
            ),
            // NOW prefix streams
            (
                "NOW: TNT SPORT 1 \u{1D3F}\u{1D2C}\u{1D42}",
                "tnt sports 1",
                "unknown",
                None,
            ),
            (
                "NOW: TNT SPORT 2 \u{1D3F}\u{1D2C}\u{1D42}",
                "tnt sports 2",
                "unknown",
                None,
            ),
            // UHD with superscript resolution (no digit after "sport" so no plural normalization)
            (
                "NOW: TNT SPORT  \u{1D41}\u{1D34}\u{1D30} \u{00B3}\u{2078}\u{2074}\u{2070}\u{1D3E}",
                "tnt sport",
                "4k",
                None,
            ),
            // VIP with superscript 4k
            (
                "VIP: TNT SPORTS ULTIMATE \u{2074}\u{1D4F} & \u{00B3}\u{2078}\u{2074}\u{2070}\u{1D3E}",
                "tnt sports ultimate &",
                "4k",
                None,
            ),
        ];

        let mut failures = Vec::new();
        for (input, exp_name, exp_quality, exp_country) in &cases {
            let result = normalize_stream_name(input);
            let mut issues = Vec::new();
            if result.normalized_name != *exp_name {
                issues.push(format!(
                    "name: got '{}' expected '{}'",
                    result.normalized_name, exp_name
                ));
            }
            if result.quality != *exp_quality {
                issues.push(format!(
                    "quality: got '{}' expected '{}'",
                    result.quality, exp_quality
                ));
            }
            if result.country.as_deref() != *exp_country {
                issues.push(format!(
                    "country: got {:?} expected {:?}",
                    result.country, exp_country
                ));
            }
            if !issues.is_empty() {
                failures.push(format!("\"{}\":\n  {}", input, issues.join("\n  ")));
            }
        }
        assert!(
            failures.is_empty(),
            "Normalization failures:\n{}",
            failures.join("\n")
        );
    }
}
