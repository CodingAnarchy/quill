//! UTF-16 position conversion (#1232): the iPhone editor addresses text in
//! UTF-16 code units, the buffer in bytes. Emoji (surrogate pairs) and
//! combining marks are where the two counts part ways.

use super::super::Buffer;

#[test]
fn ascii_positions_are_the_same_in_both() {
    let b = Buffer::from_text("plain text");
    assert_eq!(b.len_utf16(), 10);
    assert_eq!(b.byte_of_utf16(5), 5);
    assert_eq!(b.utf16_of_byte(5), 5);
}

#[test]
fn an_emoji_is_two_code_units_and_four_bytes() {
    // "a🙂b": 'a' (1 cu, 1 byte), 🙂 (2 cu, 4 bytes), 'b'.
    let b = Buffer::from_text("a🙂b");
    assert_eq!(b.len_utf16(), 4);
    assert_eq!(b.byte_of_utf16(1), 1);
    assert_eq!(b.byte_of_utf16(3), 5); // just after the emoji
    assert_eq!(b.utf16_of_byte(5), 3);
    // Inside the pair rounds down to the emoji's start: never half an emoji.
    assert_eq!(b.byte_of_utf16(2), 1);
}

#[test]
fn a_combining_mark_is_its_own_code_unit() {
    // "e\u{301}" is "é" spelled with a combining acute: 2 cu, 3 bytes.
    let b = Buffer::from_text("cafe\u{301}!");
    assert_eq!(b.len_utf16(), 6);
    assert_eq!(b.byte_of_utf16(5), 6);
    assert_eq!(b.utf16_of_byte(6), 5);
}

#[test]
fn positions_past_the_end_clamp() {
    let b = Buffer::from_text("é");
    assert_eq!(b.byte_of_utf16(99), 2);
    assert_eq!(b.utf16_of_byte(99), 1);
}

#[test]
fn an_edit_addressed_in_utf16_lands_in_the_right_place() {
    // Replace the word after an emoji, as the phone would: UTF-16 range
    // [3, 8) is "world".
    let mut b = Buffer::from_text("🙂 world!");
    let (start, end) = (b.byte_of_utf16(3), b.byte_of_utf16(8));
    b.replace_byte_range(start, end, "there");
    assert_eq!(b.text(), "🙂 there!");
}
