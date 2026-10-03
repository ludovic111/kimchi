//! Real speech through the real model. Needs the network once (the model download) and a
//! speech file: `KIMCHI_SPEECH_WAV=/path/to/16k-mono.wav KIMCHI_WHISPER_DIR=/models cargo test
//! -p kimchi-captions --test speech -- --ignored`.

use kimchi_captions::whisper::{Model, Whisper, download};
use tokio_util::sync::CancellationToken;

/// 16-bit PCM WAV → f32 samples (the JFK sample is 16 kHz mono).
fn wav(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk") + 8;
    bytes[data..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn transcribes_speech() {
    let (Ok(wav_path), Ok(root)) = (std::env::var("KIMCHI_SPEECH_WAV"), std::env::var("KIMCHI_WHISPER_DIR")) else { return };
    let root = std::path::PathBuf::from(root);
    download(&root, Model::Base, |_| {}, &CancellationToken::new()).await.unwrap();
    let samples = wav(&wav_path);
    let started = std::time::Instant::now();
    let t = tokio::task::spawn_blocking(move || {
        let mut w = Whisper::load(&Model::Base.dir(&root)).unwrap();
        w.transcribe(&samples, None, &|_| {}, &CancellationToken::new()).unwrap()
    })
    .await
    .unwrap();
    eprintln!("{:.1} s: {t:#?}", started.elapsed().as_secs_f64());
    let text: String = t.segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
    assert_eq!(t.language, "en");
    assert!(text.contains("ask not what your country can do for you"), "{text}");
    assert!(t.segments.windows(2).all(|w| w[0].end <= w[1].start + 0.5));
}
