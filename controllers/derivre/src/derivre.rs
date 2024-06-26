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

fn main() {
    let mut rx = RegexVec::new_single("[ab]c").unwrap();
    assert!(rx.is_match("ac"));
    assert!(rx.is_match("bc"));
    assert!(!rx.is_match("xxac"));
    assert!(!rx.is_match("acxx"));

    let mut rx = RegexVec::new_single("[abx]*(?P<stop>[xq]*y)").unwrap();
    assert!(rx.lookahead_len("axxxxxxxy") == Some(1));
    assert!(rx.lookahead_len("axxxxxxxqqqy") == Some(4));
    assert!(rx.lookahead_len("axxxxxxxqqq") == None);
    assert!(rx.lookahead_len("ccqy") == None);

    let mut rx = RegexVec::new_single("a[bc](de|fg)").unwrap();
    no_match(&mut rx, "abd");
    match_(&mut rx, "abde");
    look(&mut rx, "abde", Some(0));

    no_match(&mut rx, "abdea");
    println!("{:?}", rx);

    let mut rx = RegexVec::new_single("a[bc]*(de|fg)*x").unwrap();
    no_match_many(&mut rx, &["", "a", "b", "axb"]);
    match_many(&mut rx, &["ax", "abdex", "abcbcbcbcdex", "adefgdefgx"]);
    println!("{:?}", rx);
    //
    //
    //

    eprintln!("\n\n\nSTART");
    let parser = regex_syntax::ParserBuilder::new()
        // .dot_matches_new_line(false)
        // .unicode(false)
        // .utf8(false)
        .build();
    let mut rx = RegexVec::new_with_parser(parser, &["a(bc+|b[eh])g|.h"]).unwrap();
    println!("{:?}", rx);
    no_match(&mut rx, "abh");
    println!("{:?}", rx);
    rx.set_fuel(5);
    no_match(&mut rx, "abcg");
    assert!(rx.has_error());
    println!("{:?}", rx);
}
