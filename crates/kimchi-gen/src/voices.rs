//! Voice lists that several providers share: the same model family hosted by its maker, by fal
//! and by Replicate reads with the same voices. Providers whose API lists voices (ElevenLabs,
//! MiniMax) fetch them instead and fall back to these.

use crate::types::Voice;

/// `(id, description)` → voices, the id doubling as the name.
fn named(list: &[(&str, &str)], gender_of: impl Fn(&str) -> Option<&'static str>) -> Vec<Voice> {
    list.iter()
        .map(|(id, d)| {
            let mut v = Voice::new(*id, id.replace('_', " "));
            if !d.is_empty() {
                v.description = Some((*d).to_string());
            }
            v.gender = gender_of(id).map(str::to_string);
            v
        })
        .collect()
}

/// Gemini's 30 prebuilt speech voices with the one word Google gives each (ai.google.dev,
/// "Speech generation", voice options). They speak every language Gemini speech does.
pub const GEMINI: &[(&str, &str)] = &[
    ("Kore", "Firm"),
    ("Puck", "Upbeat"),
    ("Zephyr", "Bright"),
    ("Charon", "Informative"),
    ("Fenrir", "Excitable"),
    ("Leda", "Youthful"),
    ("Orus", "Firm"),
    ("Aoede", "Breezy"),
    ("Callirrhoe", "Easy-going"),
    ("Autonoe", "Bright"),
    ("Enceladus", "Breathy"),
    ("Iapetus", "Clear"),
    ("Umbriel", "Easy-going"),
    ("Algieba", "Smooth"),
    ("Despina", "Smooth"),
    ("Erinome", "Clear"),
    ("Algenib", "Gravelly"),
    ("Rasalgethi", "Informative"),
    ("Laomedeia", "Upbeat"),
    ("Achernar", "Soft"),
    ("Alnilam", "Firm"),
    ("Schedar", "Even"),
    ("Gacrux", "Mature"),
    ("Pulcherrima", "Forward"),
    ("Achird", "Friendly"),
    ("Zubenelgenubi", "Casual"),
    ("Vindemiatrix", "Gentle"),
    ("Sadachbia", "Lively"),
    ("Sadaltager", "Knowledgeable"),
    ("Sulafat", "Warm"),
];

pub fn gemini() -> Vec<Voice> {
    named(GEMINI, |_| None).into_iter().map(|v| v.language("multilingual")).collect()
}

/// ElevenLabs' default voices, which fal and Replicate accept by name. (The ElevenLabs provider
/// lists the account's voices, with samples, from its API.)
pub const ELEVENLABS: &[(&str, &str, &str)] = &[
    ("Rachel", "Calm, American", "female"),
    ("Aria", "Expressive, American", "female"),
    ("Sarah", "Soft, American", "female"),
    ("Laura", "Upbeat, American", "female"),
    ("Charlotte", "Warm, Swedish", "female"),
    ("Alice", "Confident, British", "female"),
    ("Matilda", "Friendly, American", "female"),
    ("Jessica", "Expressive, American", "female"),
    ("Lily", "Warm, British", "female"),
    ("Roger", "Confident, American", "male"),
    ("Charlie", "Natural, Australian", "male"),
    ("George", "Warm, British", "male"),
    ("Callum", "Intense, Transatlantic", "male"),
    ("Liam", "Articulate, American", "male"),
    ("Will", "Friendly, American", "male"),
    ("Eric", "Friendly, American", "male"),
    ("Chris", "Casual, American", "male"),
    ("Brian", "Deep, American", "male"),
    ("Daniel", "Authoritative, British", "male"),
    ("Bill", "Trustworthy, American", "male"),
    ("River", "Confident, American", "neutral"),
];

pub fn elevenlabs() -> Vec<Voice> {
    ELEVENLABS.iter().map(|(n, d, g)| Voice::new(*n, *n).described(*d).gender(*g)).collect()
}

/// MiniMax's system voices that its speech models take by id (as fal and Replicate document
/// them; the MiniMax provider asks the API for the full list).
pub const MINIMAX: &[(&str, &str)] = &[
    ("Wise_Woman", "Measured and kind"),
    ("Friendly_Person", "Warm and approachable"),
    ("Inspirational_girl", "Bright and encouraging"),
    ("Deep_Voice_Man", "Low and steady"),
    ("Calm_Woman", "Soft and calm"),
    ("Casual_Guy", "Relaxed"),
    ("Lively_Girl", "Energetic"),
    ("Patient_Man", "Patient"),
    ("Young_Knight", "Young and bold"),
    ("Determined_Man", "Determined"),
    ("Lovely_Girl", "Sweet"),
    ("Decent_Boy", "Polite, young"),
    ("Imposing_Manner", "Commanding"),
    ("Elegant_Man", "Refined"),
    ("Abbess", "Solemn"),
    ("Sweet_Girl_2", "Sweet"),
    ("Exuberant_Girl", "Exuberant"),
];

pub fn minimax() -> Vec<Voice> {
    named(MINIMAX, |id| {
        let l = id.to_ascii_lowercase();
        if ["woman", "girl", "abbess"].iter().any(|k| l.contains(k)) {
            Some("female")
        } else if ["man", "guy", "boy", "knight"].iter().any(|k| l.contains(k)) {
            Some("male")
        } else {
            None
        }
    })
}

/// Kokoro-82M's voices: the prefix says the language (a American, b British English, e Spanish,
/// f French, h Hindi, i Italian, j Japanese, p Portuguese, z Mandarin) and f/m the gender.
pub const KOKORO: &[&str] = &[
    "af_heart", "af_alloy", "af_aoede", "af_bella", "af_jessica", "af_kore", "af_nicole", "af_nova", "af_river", "af_sarah", "af_sky", "am_adam",
    "am_echo", "am_eric", "am_fenrir", "am_liam", "am_michael", "am_onyx", "am_puck", "bf_alice", "bf_emma", "bf_isabella", "bf_lily", "bm_daniel",
    "bm_fable", "bm_george", "bm_lewis", "ff_siwis", "hf_alpha", "hf_beta", "hm_omega", "hm_psi", "if_sara", "im_nicola", "jf_alpha",
    "jf_gongitsune", "jf_nezumi", "jf_tebukuro", "jm_kumo", "zf_xiaobei", "zf_xiaoni", "zf_xiaoxiao", "zf_xiaoyi", "zm_yunjian", "zm_yunxi",
    "zm_yunxia", "zm_yunyang",
];

pub fn kokoro(only_american: bool) -> Vec<Voice> {
    KOKORO
        .iter()
        .filter(|id| !only_american || id.starts_with('a'))
        .map(|id| {
            let mut chars = id.chars();
            let (lang, gender) = (chars.next().unwrap_or('a'), chars.next().unwrap_or('f'));
            let language = match lang {
                'a' => "American English",
                'b' => "British English",
                'e' => "Spanish",
                'f' => "French",
                'h' => "Hindi",
                'i' => "Italian",
                'j' => "Japanese",
                'p' => "Portuguese",
                _ => "Mandarin",
            };
            let name = id.split_once('_').map(|(_, n)| n).unwrap_or(id);
            let mut c = name.chars();
            let name = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
            Voice::new(*id, name).language(language).gender(if gender == 'm' { "male" } else { "female" })
        })
        .collect()
}

/// xAI's speech voices (Grok's TTS), lower-case ids.
pub const XAI: &[&str] = &[
    "eve", "ara", "leo", "rex", "sal", "carina", "zagan", "helix", "orion", "luna", "iris", "altair", "zenith", "perseus", "helios", "lux", "kepler",
    "rigel", "cosmo", "celeste", "ursa", "sirius", "lumen", "castor", "naksh", "atlas", "aurora", "liora",
];

pub fn xai() -> Vec<Voice> {
    XAI.iter()
        .map(|id| {
            let mut c = id.chars();
            Voice::new(*id, c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()).language("multilingual")
        })
        .collect()
}

/// Qwen3-TTS speakers.
pub const QWEN: &[(&str, &str)] = &[
    ("Vivian", "Bright, Chinese and English"),
    ("Serena", "Gentle, Chinese and English"),
    ("Uncle_Fu", "Seasoned, Chinese"),
    ("Dylan", "Youthful, Beijing Chinese"),
    ("Eric", "Lively, Sichuan Chinese"),
    ("Ryan", "Dynamic, English"),
    ("Aiden", "Sunny, American English"),
    ("Ono_Anna", "Playful, Japanese"),
    ("Sohee", "Warm, Korean"),
];

pub fn qwen() -> Vec<Voice> {
    named(QWEN, |_| None)
}

/// The language name MiniMax and Qwen take (`language_boost: "English"`) for an ISO code.
pub fn language_name(code: &str) -> Option<&'static str> {
    let code = code.trim().to_ascii_lowercase();
    let base = code.split(['-', '_']).next().unwrap_or("");
    Some(match base {
        "auto" => "auto",
        "en" => "English",
        "zh" => "Chinese",
        "yue" => "Chinese,Yue",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "ru" => "Russian",
        "ar" => "Arabic",
        "nl" => "Dutch",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        "id" => "Indonesian",
        "th" => "Thai",
        "pl" => "Polish",
        "ro" => "Romanian",
        "el" => "Greek",
        "cs" => "Czech",
        "fi" => "Finnish",
        "hi" => "Hindi",
        "sv" => "Swedish",
        "da" => "Danish",
        "he" => "Hebrew",
        "hu" => "Hungarian",
        "no" | "nb" => "Norwegian",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists() {
        assert_eq!(gemini().len(), 30);
        assert_eq!(gemini()[0].description.as_deref(), Some("Firm"));
        let k = kokoro(false);
        assert_eq!(k.len(), KOKORO.len());
        let heart = &k[0];
        assert_eq!((heart.id.as_str(), heart.name.as_str(), heart.gender.as_deref()), ("af_heart", "Heart", Some("female")));
        assert!(kokoro(true).iter().all(|v| v.language.as_deref() == Some("American English")));
        assert_eq!(minimax().iter().find(|v| v.id == "Deep_Voice_Man").and_then(|v| v.gender.clone()).as_deref(), Some("male"));
        assert_eq!(language_name("en-US"), Some("English"));
        assert_eq!(language_name("xx"), None);
    }
}
