use derivre::RegexVec;

fn check_is_match(rx: &mut RegexVec, s: &str, exp: bool) {
    if rx.is_match(s) == exp {
    } else {
        panic!(
            "error for: {:?}; expected {}",
            s,
            if exp { "match" } else { "no match" }
        );
    }
}

fn match_(rx: &mut RegexVec, s: &str) {
    check_is_match(rx, s, true);
}

fn match_many(rx: &mut RegexVec, ss: &[&str]) {
    for s in ss {
        match_(rx, s);
    }
}

fn no_match(rx: &mut RegexVec, s: &str) {
    check_is_match(rx, s, false);
}

fn no_match_many(rx: &mut RegexVec, ss: &[&str]) {
    for s in ss {
        no_match(rx, s);
    }
}

fn look(rx: &mut RegexVec, s: &str, exp: Option<usize>) {
    let res = rx.lookahead_len(s);
    if res == exp {
    } else {
        panic!(
            "lookahead len error for: {:?}; expected {:?}, got {:?}",
            s, exp, res
        )
    }
}

#[test]
fn test_basic() {
    let mut rx = RegexVec::new_single("a[bc](de|fg)").unwrap();
    println!("{:?}", rx);
    no_match(&mut rx, "abd");
    match_(&mut rx, "abde");

    no_match(&mut rx, "abdea");
    println!("{:?}", rx);

    let mut rx = RegexVec::new_single("a[bc]*(de|fg)*x").unwrap();

    no_match_many(&mut rx, &["", "a", "b", "axb"]);
    match_many(&mut rx, &["ax", "abdex", "abcbcbcbcdex", "adefgdefgx"]);
    println!("{:?}", rx);

    let mut rx = RegexVec::new_single("(A|foo)*").unwrap();
    match_many(
        &mut rx,
        &["", "A", "foo", "Afoo", "fooA", "foofoo", "AfooA", "Afoofoo"],
    );

    let mut rx = RegexVec::new_single("[abcquv][abdquv]").unwrap();
    match_many(
        &mut rx,
        &["aa", "ab", "ba", "ca", "cd", "ad", "aq", "qa", "qd"],
    );
    no_match_many(&mut rx, &["cc", "dd", "ac", "ac", "bc"]);

    println!("{:?}", rx);

    let mut rx = RegexVec::new_single("ab{3,5}c").unwrap();
    match_many(&mut rx, &["abbbc", "abbbbc", "abbbbbc"]);
    no_match_many(
        &mut rx,
        &["", "ab", "abc", "abbc", "abbb", "abbbx", "abbbbbbc"],
    );

    let mut rx = RegexVec::new_single("x*A[0-9]{5}").unwrap();
    match_many(&mut rx, &["A12345", "xxxxxA12345", "xA12345"]);
    no_match_many(&mut rx, &["A1234", "xxxxxA123456", "xA123457"]);
}

#[test]
fn test_unicode() {
    let mut rx = RegexVec::new_single("źółw").unwrap();
    println!("{:?}", rx);
    no_match(&mut rx, "zolw");
    match_(&mut rx, "źółw");
    no_match(&mut rx, "źół");
    println!("{:?}", rx);

    let mut rx = RegexVec::new_single("[źó]łw").unwrap();
    match_(&mut rx, "ółw");
    match_(&mut rx, "źłw");
    no_match(&mut rx, "źzłw");

    let mut rx = RegexVec::new_single("x[©ª«]y").unwrap();
    match_many(&mut rx, &["x©y", "xªy", "x«y"]);
    no_match_many(&mut rx, &["x®y", "x¶y", "x°y", "x¥y"]);

    let mut rx = RegexVec::new_single("x[ab«\u{07ff}\u{0800}]y").unwrap();
    match_many(&mut rx, &["xay", "xby", "x«y", "x\u{07ff}y", "x\u{0800}y"]);
    no_match_many(&mut rx, &["xcy", "xªy", "x\u{07fe}y", "x\u{0801}y"]);

    let mut rx = RegexVec::new_single("x[ab«\u{07ff}-\u{0801}]y").unwrap();
    match_many(
        &mut rx,
        &[
            "xay",
            "xby",
            "x«y",
            "x\u{07ff}y",
            "x\u{0800}y",
            "x\u{0801}y",
        ],
    );
    no_match_many(&mut rx, &["xcy", "xªy", "x\u{07fe}y", "x\u{0802}y"]);

    let mut rx = RegexVec::new_single(".").unwrap();
    no_match(&mut rx, "\n");
    match_many(&mut rx, &["a", "1", " ", "\r"]);

    let mut rx = RegexVec::new_single("a.*b").unwrap();
    match_many(&mut rx, &["ab", "a123b", "a \r\t123b"]);
    no_match_many(&mut rx, &["a", "a\nb", "a1\n2b"]);
}

#[test]
fn test_lookaround() {
    let mut rx = RegexVec::new_single("[ab]*(?P<stop>xx)").unwrap();
    match_(&mut rx, "axx");
    look(&mut rx, "axx", Some(2));
    look(&mut rx, "ax", None);

    let mut rx = RegexVec::new_single("[ab]*(?P<stop>x*y)").unwrap();
    look(&mut rx, "axy", Some(2));
    look(&mut rx, "ay", Some(1));
    look(&mut rx, "axxy", Some(3));
    look(&mut rx, "aaaxxy", Some(3));
    look(&mut rx, "abaxxy", Some(3));
    no_match_many(&mut rx, &["ax", "bx", "aaayy", "axb", "axyxx"]);

    let mut rx = RegexVec::new_single("[abx]*(?P<stop>[xq]*y)").unwrap();
    look(&mut rx, "axxxxxxxy", Some(1));
    look(&mut rx, "axxxxxxxqy", Some(2));
    look(&mut rx, "axxxxxxxqqqy", Some(4));

    let mut rx = RegexVec::new_single("(f|foob)(?P<stop>o*y)").unwrap();
    look(&mut rx, "fooby", Some(1));
    look(&mut rx, "fooy", Some(3));
    look(&mut rx, "fy", Some(1));
}

#[test]
fn test_fuel() {
    let mut rx = RegexVec::new_single("a(bc+|b[eh])g|.h").unwrap();
    println!("{:?}", rx);
    rx.set_fuel(200);
    match_(&mut rx, "abcg");
    assert!(!rx.has_error());

    let mut rx = RegexVec::new_single("a(bc+|b[eh])g|.h").unwrap();
    println!("{:?}", rx);
    rx.set_fuel(20);
    no_match(&mut rx, "abcg");
    assert!(rx.has_error());
}

#[test]
fn utf8_dfa() {
    let parser = regex_syntax::ParserBuilder::new()
        .unicode(false)
        .utf8(false)
        .ignore_whitespace(true)
        .build();

    let utf8_rx = r#"
   ( [\x00-\x7F]                        # ASCII
   | [\xC2-\xDF][\x80-\xBF]             # non-overlong 2-byte
   |  \xE0[\xA0-\xBF][\x80-\xBF]        # excluding overlongs
   | [\xE1-\xEC\xEE\xEF][\x80-\xBF]{2}  # straight 3-byte
   |  \xED[\x80-\x9F][\x80-\xBF]        # excluding surrogates
   |  \xF0[\x90-\xBF][\x80-\xBF]{2}     # planes 1-3
   | [\xF1-\xF3][\x80-\xBF]{3}          # planes 4-15
   |  \xF4[\x80-\x8F][\x80-\xBF]{2}     # plane 16
   )*
   "#;

    let mut rx = RegexVec::new_with_parser(parser, &[utf8_rx]).unwrap();
    println!("UTF8 {:?}", rx);
    //match_many(&mut rx, &["a", "ą", "ę", "ó", "≈ø¬", "\u{1f600}"]);
    println!("UTF8 {:?}", rx);
    let compiled = rx.dfa();
    println!("UTF8 {:?}", rx);
    println!("mapping ({}) {:?}", rx.alphabet_size(), &compiled[0..256]);
    println!("states {:?}", &compiled[256..]);
    println!("initial {:?}", rx.initial_state_all());
}

#[test]
fn utf8_restrictions() {
    let mut rx = RegexVec::new_single("(.|\n)*").unwrap();
    println!("{:?}", rx);
    match_many(&mut rx, &["", "a", "\n", "\n\n", "\x00", "\x7f"]);
    let s0 = rx.initial_state_all();
    assert!(rx.transition(s0, 0x80).is_dead());
    assert!(rx.transition(s0, 0xC0).is_dead());
    assert!(rx.transition(s0, 0xC1).is_dead());
    // more overlong:
    assert!(rx.transition_bytes(s0, &[0xE0, 0x80]).is_dead());
    assert!(rx.transition_bytes(s0, &[0xE0, 0x9F]).is_dead());
    assert!(rx.transition_bytes(s0, &[0xF0, 0x80]).is_dead());
    assert!(rx.transition_bytes(s0, &[0xF0, 0x8F]).is_dead());
    // surrogates:
    assert!(rx.transition_bytes(s0, &[0xED, 0xA0]).is_dead());
    assert!(rx.transition_bytes(s0, &[0xED, 0xAF]).is_dead());
    assert!(rx.transition_bytes(s0, &[0xED, 0xBF]).is_dead());
}

#[test]
fn trie() {
    let mut rx = RegexVec::new_single("(foo|far|bar|baz)").unwrap();
    match_many(&mut rx, &["foo", "far", "bar", "baz"]);
    no_match_many(&mut rx, &["fo", "fa", "b", "ba", "baa", "f", "faz"]);

    let mut rx = RegexVec::new_single("(foobarbazqux123|foobarbazqux124)").unwrap();
    match_many(&mut rx, &["foobarbazqux123", "foobarbazqux124"]);
    no_match_many(
        &mut rx,
        &["foobarbazqux12", "foobarbazqux125", "foobarbazqux12x"],
    );

    let mut rx = RegexVec::new_single("(1a|12a|123a|1234a|12345a|123456a)").unwrap();
    match_many(
        &mut rx,
        &["1a", "12a", "123a", "1234a", "12345a", "123456a"],
    );
    no_match_many(
        &mut rx,
        &["1234567a", "123456", "12345", "1234", "123", "12", "1"],
    );
}

#[test]
fn unicode_case() {
    let mut rx = RegexVec::new_single("(?i)Żółw").unwrap();
    match_many(&mut rx, &["Żółw", "żółw", "ŻÓŁW", "żóŁw"]);
    no_match_many(&mut rx, &["zółw"]);

    let mut rx = RegexVec::new_single("Żółw").unwrap();
    match_(&mut rx, "Żółw");
    no_match_many(&mut rx, &["żółw", "ŻÓŁW", "żóŁw"]);
}