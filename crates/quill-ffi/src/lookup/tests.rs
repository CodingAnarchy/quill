use super::*;

#[test]
fn without_a_key_the_lookup_says_what_is_missing() {
    // Test builds never read the keychain or the environment, so no key.
    assert!(!has_claude_api_key());
    let err = look_up_words("gloomy", "The night was gloomy.").unwrap_err();
    assert!(err.to_string().contains("API key in Settings"), "{err}");
}

#[test]
fn only_a_word_or_short_phrase_can_be_looked_up() {
    for selection in ["", "   ", "a whole run of words in a sentence"] {
        let err = look_up_words(selection, "").unwrap_err();
        assert!(
            err.to_string().contains("short phrase"),
            "{selection:?}: {err}"
        );
    }
}

#[test]
fn failures_read_as_what_to_do() {
    let auth = lookup_error(AiError::Network {
        status: Some(401),
        message: "unauthorized".into(),
    });
    assert!(auth.to_string().contains("API key"), "{auth}");
    let offline = lookup_error(AiError::Network {
        status: None,
        message: "dns".into(),
    });
    assert!(offline.to_string().contains("connection"), "{offline}");
}

#[test]
fn candidates_carry_the_words_details() {
    let words = thesaurus::parse(
        r#"{"words":[{"text":"somber","definition":"dark and gloomy","syllables":2,"origin":"Latinate","rarity":"common"}]}"#,
    )
    .unwrap();
    let candidate: WordCandidate = words.into_iter().next().unwrap().into();
    assert_eq!(candidate.text, "somber");
    assert_eq!(candidate.syllables, Some(2));
    assert_eq!(candidate.origin.as_deref(), Some("Latinate"));
}
