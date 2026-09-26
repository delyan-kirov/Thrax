use super::*;

fn key(bytes: &[u8]) -> Option<Key> {
    let mut input = bytes;
    term::read_key(&mut input)
}

#[test]
fn emacs_control_keys_decode() {
    assert_eq!(key(&[0x01]), Some(Key::Home)); // Ctrl-A
    assert_eq!(key(&[0x05]), Some(Key::End)); // Ctrl-E
    assert_eq!(key(&[0x02]), Some(Key::Left)); // Ctrl-B
    assert_eq!(key(&[0x06]), Some(Key::Right)); // Ctrl-F
    assert_eq!(key(&[0x0b]), Some(Key::KillToEol)); // C-k
    assert_eq!(key(&[0x04]), Some(Key::CtrlD));
    assert_eq!(key(&[0x7f]), Some(Key::Backspace));
    assert_eq!(key(&[b'\r']), Some(Key::Enter));
}

#[test]
fn arrow_escapes_decode_and_others_are_skipped() {
    assert_eq!(key(b"\x1b[C"), Some(Key::Right));
    assert_eq!(key(b"\x1b[D"), Some(Key::Left));
    assert_eq!(key(b"\x1bOH"), Some(Key::Home));
    assert_eq!(key(b"\x1b[F"), Some(Key::End));
    // A modified arrow still resolves by its final byte.
    assert_eq!(key(b"\x1b[1;5C"), Some(Key::Right));
    // An unrecognized sequence is swallowed, so the following key surfaces.
    assert_eq!(key(b"\x1b[3~a"), Some(Key::Char('a')));
    // Vertical arrows and the Alt-word motions.
    assert_eq!(key(b"\x1b[A"), Some(Key::Up));
    assert_eq!(key(b"\x1b[B"), Some(Key::Down));
    assert_eq!(key(b"\x1bb"), Some(Key::WordLeft)); // M-b
    assert_eq!(key(b"\x1bf"), Some(Key::WordRight)); // M-f
    assert_eq!(key(b"\x1bd"), Some(Key::KillWordForward)); // M-d
    assert_eq!(key(b"\x1b\x7f"), Some(Key::KillWordBack)); // M-DEL
    assert_eq!(key(&[0x17]), Some(Key::KillWordBack)); // C-w
    assert_eq!(key(&[0x10]), Some(Key::Up)); // C-p
    assert_eq!(key(&[0x0e]), Some(Key::Down)); // C-n
    // C-u is universal-argument in emacs, not a kill: it is ignored here, so the
    // next key surfaces.
    assert_eq!(key(&[0x15, b'z']), Some(Key::Char('z')));
    assert_eq!(key(&[0x16]), Some(Key::PageDown)); // C-v
    assert_eq!(key(b"\x1bv"), Some(Key::PageUp)); // M-v
    assert_eq!(key(b"\x1b<"), Some(Key::BufferStart)); // M-<
    assert_eq!(key(b"\x1b>"), Some(Key::BufferEnd)); // M->
    assert_eq!(key(&[0x0c]), Some(Key::ClearScreen)); // C-l
    assert_eq!(key(&[0x1f]), Some(Key::Undo)); // C-/ (and C-_)
    assert_eq!(key(b"\x1b/"), Some(Key::Redo)); // M-/
    assert_eq!(key(&[0x12]), Some(Key::HistorySearch)); // C-r
    assert_eq!(key(&[0x07]), Some(Key::Escape)); // C-g cancels
    assert_eq!(key(&[0x1a]), Some(Key::Suspend)); // C-z suspends the shell
    // A lone ESC (no byte waiting) is Escape, not the start of a sequence.
    assert_eq!(key(&[0x1b]), Some(Key::Escape));
}

#[test]
fn multibyte_utf8_decodes_to_one_char() {
    assert_eq!(key("é".as_bytes()), Some(Key::Char('é')));
    assert_eq!(key("λ".as_bytes()), Some(Key::Char('λ')));
    assert_eq!(key("🦀".as_bytes()), Some(Key::Char('🦀')));
    assert_eq!(key(b"A"), Some(Key::Char('A')));
}

#[test]
fn return_and_ctrl_j_split_into_enter_and_submit() {
    // With ICRNL off, the Return key is CR and Ctrl-J is LF: Return edits an item,
    // Ctrl-J evaluates the whole buffer.
    assert_eq!(key(b"\r"), Some(Key::Enter));
    assert_eq!(key(b"\n"), Some(Key::SubmitAll)); // Ctrl-J
}

#[test]
fn ctrl_j_submits_the_whole_buffer_without_a_terminator() {
    // A multi-line buffer with no trailing `$` yields the whole thing, trimmed.
    let ed = editor_with("_= foo\n  + bar");
    assert_eq!(ed.submit_body(), "_= foo\n  + bar");
    // A lone trailing `$` terminator is dropped so it is not passed on.
    let ed = editor_with("_= 1 + 2 $");
    assert_eq!(ed.submit_body(), "_= 1 + 2");
}

fn editor_with(buffer: &str) -> Editor {
    let mut ed = Editor::new();
    ed.set_buffer(buffer.to_string());
    ed
}

#[test]
fn insert_respects_cursor_position() {
    let mut ed = editor_with("ac");
    ed.move_left();
    ed.insert('b');
    assert_eq!(ed.buffer, "abc");
    assert_eq!(ed.cursor, 2);
}

#[test]
fn backspace_and_forward_delete() {
    let mut ed = editor_with("abc");
    ed.backspace();
    assert_eq!((ed.buffer.as_str(), ed.cursor), ("ab", 2));

    let mut ed = editor_with("abc");
    ed.move_home();
    ed.delete_forward();
    assert_eq!((ed.buffer.as_str(), ed.cursor), ("bc", 0));
}

#[test]
fn home_and_end_are_line_wise() {
    let mut ed = editor_with("ab\ncd");
    ed.move_home();
    assert_eq!(ed.cursor, 3);
    ed.move_end();
    assert_eq!(ed.cursor, 5);
}

#[test]
fn kill_operations() {
    let mut ed = editor_with("hello");
    ed.move_home();
    ed.move_right();
    ed.move_right();
    ed.kill_to_eol();
    assert_eq!((ed.buffer.as_str(), ed.cursor), ("he", 2));

    let mut ed = editor_with("foo bar");
    ed.kill_word_back();
    assert_eq!((ed.buffer.as_str(), ed.cursor), ("foo ", 4));

    let mut ed = editor_with("foo bar");
    ed.move_home();
    ed.kill_word_forward();
    assert_eq!((ed.buffer.as_str(), ed.cursor), (" bar", 0));
}

#[test]
fn word_motion_skips_then_crosses_a_word() {
    let mut ed = editor_with("foo bar baz");
    ed.move_word_left(); // from end, back over "baz"
    assert_eq!(ed.cursor, 8);
    ed.move_word_left(); // back over "bar"
    assert_eq!(ed.cursor, 4);

    let mut ed = editor_with("foo bar");
    ed.move_home();
    ed.move_word_right(); // to end of "foo"
    assert_eq!(ed.cursor, 3);
    ed.move_word_right(); // to end of "bar"
    assert_eq!(ed.cursor, 7);
}

#[test]
fn vertical_motion_preserves_column_and_clamps() {
    let mut ed = editor_with("abcd\nxy\nwxyz");
    // cursor at end (byte 12), column 4 on the last line.
    ed.move_up(); // onto "xy" (len 2): column 4 clamps to end of line
    assert_eq!(ed.cursor, 7);
    ed.move_up(); // onto "abcd" at column 2
    assert_eq!(ed.cursor, 2);
    ed.move_down(); // back onto "xy" at column 2
    assert_eq!(ed.cursor, 7);
}

#[test]
fn vertical_motion_stops_at_edges() {
    let mut ed = editor_with("a\nb");
    ed.move_home(); // start of "b"
    ed.move_down(); // already last line: no move
    assert_eq!(ed.cursor, 2);
    ed.move_up(); // onto "a"
    assert_eq!(ed.cursor, 0);
    ed.move_up(); // already first line: no move
    assert_eq!(ed.cursor, 0);
}

#[test]
fn buffer_ends_and_paging() {
    let mut ed = editor_with("ab\ncd\nef");
    ed.move_buffer_start();
    assert_eq!(ed.cursor, 0);
    ed.move_buffer_end();
    assert_eq!(ed.cursor, 8);

    // A page taller than the item lands on the first/last line, keeping the
    // column (clamped). From column 2 on "yz", paging up reaches "abcd" col 2.
    let mut ed = editor_with("abcd\nx\nyz");
    ed.move_lines(-100);
    assert_eq!(ed.cursor, 2);
    ed.move_lines(100); // back down past the bottom: "yz" col 2 (its end)
    assert_eq!(ed.cursor, 9);
}

#[test]
fn cursor_advances_by_whole_scalar() {
    let mut ed = editor_with("");
    ed.insert('é');
    assert_eq!(ed.cursor, 2);
    ed.backspace();
    assert_eq!((ed.buffer.as_str(), ed.cursor), ("", 0));
}

#[test]
fn undo_coalesces_typing_and_redo_restores() {
    let mut ed = editor_with("");
    ed.insert('a');
    ed.insert('b');
    ed.insert('c');
    ed.undo(); // one step removes the whole typed run
    assert_eq!(ed.buffer, "");
    ed.redo();
    assert_eq!(ed.buffer, "abc");
}

#[test]
fn movement_splits_undo_steps() {
    let mut ed = editor_with("");
    ed.insert('a');
    ed.move_left();
    ed.insert('b'); // "ba": a separate step from the first insert
    assert_eq!(ed.buffer, "ba");
    ed.undo();
    assert_eq!(ed.buffer, "a");
    ed.undo();
    assert_eq!(ed.buffer, "");
}

#[test]
fn edit_after_undo_drops_redo() {
    let mut ed = editor_with("");
    ed.insert('a');
    ed.undo();
    ed.insert('b'); // branches the timeline; redo is now empty
    ed.redo();
    assert_eq!(ed.buffer, "b");
}

#[test]
fn history_search_finds_navigates_and_cancels() {
    let history = vec![
        "1 + 1".to_string(),
        "x = 5".to_string(),
        "1 + 2".to_string(),
    ];
    // Type "1 +" and accept the newest match.
    let mut input: &[u8] = b"1 +\r";
    assert_eq!(history_search(&mut input, &history), Some("1 + 2".to_string()));
    // C-r (0x12) steps to the older match before accepting.
    let mut input: &[u8] = b"1 +\x12\r";
    assert_eq!(history_search(&mut input, &history), Some("1 + 1".to_string()));
    // Escape cancels, returning nothing.
    let mut input: &[u8] = b"1 +\x1b";
    assert_eq!(history_search(&mut input, &history), None);
    // A query with no match accepts nothing.
    let mut input: &[u8] = b"zzz\r";
    assert_eq!(history_search(&mut input, &history), None);
}
