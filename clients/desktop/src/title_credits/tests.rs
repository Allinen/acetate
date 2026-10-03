use uuid::Uuid;

use super::setting::Kind;
use super::*;

const F: &str = "featured";
const P: &str = "producer";
const M: &str = "primary";

fn role(typed: &str) -> Option<String> {
    ["primary", "featured", "producer"].iter().find(|r| r.starts_with(typed)).map(|r| r.to_string())
}

fn setting(text: &str) -> Setting {
    Setting::parse(text, &role).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn with_names(rules: &str, title: &str, names: &NameTable) -> Parsed {
    parse(title, &setting(rules), names, &Fixes::default())
}

/// `title` parsed with `rules` comes out as `want` with `credits` (name, role).
#[track_caller]
fn case(rules: &str, title: &str, want: &str, credits: &[(&str, &str)]) {
    let parsed = with_names(rules, title, &NameTable::default());
    let got: Vec<(&str, &str)> = parsed.credits.iter().map(|c| (c.name.as_str(), c.role.as_str())).collect();
    assert_eq!((parsed.title.as_str(), got.as_slice()), (want, credits), "{rules}: “{title}”");
}

#[track_caller]
fn warns(rules: &str, title: &str) -> Vec<Warning> {
    with_names(rules, title, &NameTable::default()).warnings
}

#[test]
fn the_plans_examples() {
    let r = "feat prod [prod]";
    case(r, "Song (feat. Ann, Bo & Cy)", "Song", &[("Ann", F), ("Bo", F), ("Cy", F)]);
    case(r, "Song feat. Ann [prod. Dee]", "Song", &[("Dee", P), ("Ann", F)]);
    case(r, "Another nice song [bang]", "Another nice song", &[("bang", P)]);
    case(r, "Cool song! [bong]", "Cool song!", &[("bong", P)]);
    case(r, "Song (Remix) [prod. Dee]", "Song (Remix)", &[("Dee", P)]);
    case(r, "Song (feat. Ann", "Song", &[("Ann", F)]);
    assert_eq!(warns(r, "Song (feat. Ann"), [Warning::Unclosed]);
}

#[test]
fn where_a_credit_ends() {
    let r = "feat prod";
    // In brackets: to the closing one, brackets and all.
    case(r, "Song [prod. Dee] (Remix)", "Song (Remix)", &[("Dee", P)]);
    case(r, "[prod. Dee] Song", "Song", &[("Dee", P)]);
    // Another keyword in the same bracket: each runs to it, the bracket goes once empty.
    case(r, "Song (feat. Ann prod. Dee)", "Song", &[("Dee", P), ("Ann", F)]);
    // Not right after the bracket: what's before it stays in it.
    case(r, "Song (Bo Remix feat. Ann)", "Song (Bo Remix)", &[("Ann", F)]);
    // Not in brackets: to an opening bracket, a keyword, a dash, the end.
    case(r, "Song feat. Ann & Bo (Remix)", "Song (Remix)", &[("Ann", F), ("Bo", F)]);
    case(r, "Song feat. Ann prod. Dee", "Song", &[("Dee", P), ("Ann", F)]);
    case(r, "Song feat. Ann - Live", "Song - Live", &[("Ann", F)]);
    case(r, "Song - feat. Ann", "Song", &[("Ann", F)]);
    // More than one.
    case(r, "Song (feat. Ann) [feat. Bo]", "Song", &[("Ann", F), ("Bo", F)]);
    case(r, "Song (feat. Ann feat. Bo)", "Song", &[("Ann", F), ("Bo", F)]);
    // Full-width brackets.
    case(r, "歌（feat. Ann）", "歌", &[("Ann", F)]);
}

#[test]
fn the_keywords() {
    case("feat", "Song ft. Ann", "Song", &[("Ann", F)]);
    case("feat", "Song FT Ann", "Song", &[("Ann", F)]);
    case("feat", "Song featuring Ann", "Song", &[("Ann", F)]);
    case("prod", "Song (produced by Dee)", "Song", &[("Dee", P)]);
    case("prod", "Song prod by Dee", "Song", &[("Dee", P)]);
    case("prod", "Song [Prod. By Dee]", "Song", &[("Dee", P)]);
    case("with", "Song w/ Ann", "Song", &[("Ann", F)]);
    case("with", "Song (with Ann)", "Song", &[("Ann", F)]);
    case("with", "Song w/o Ann", "Song w/o Ann", &[]);
    case("w", "Song w Ann", "Song", &[("Ann", F)]);
    case("x", "Song x Ann", "Song", &[("Ann", F)]);
    // Roles changed.
    case("with=prod", "Song w/ Dee", "Song", &[("Dee", P)]);
    case("x=primary", "Song x Ann", "Song", &[("Ann", M)]);
    // Your own.
    case("word:vs=featured", "Song (vs Ann)", "Song", &[("Ann", F)]);
    case("word:\"beat by\"=producer", "Song beat by Dee", "Song", &[("Dee", P)]);
}

#[test]
fn only_whole_words_and_not_first() {
    let all = "feat prod with w x";
    for title in ["Feathers", "Product of Swan", "Max Exodus", "Wwwhat", "Featherweight (Prodigy)"] {
        case(all, title, title, &[]);
    }
    // Something has to come before a keyword.
    case("with", "With You", "With You", &[]);
    case("x", "X Song", "X Song", &[]);
    // A keyword with nothing after it is left (and stays flagged).
    case("feat", "Song feat.", "Song feat.", &[]);
    assert_eq!(warns("feat", "Song feat."), [Warning::KeywordLeft]);
}

#[test]
fn plus() {
    case("+feat", "Song + Bo", "Song", &[("Bo", F)]);
    case("+prod", "Song + Bo", "Song", &[("Bo", P)]);
    case("+=prod", "Song (+ Bo)", "Song", &[("Bo", P)]);
    case("+feat", "C++ Song", "C++ Song", &[]);
    // The keywords go first: a + among their names is a separator.
    case("feat +prod", "Song feat. Ann + Bo", "Song", &[("Ann", F), ("Bo", F)]);
}

#[test]
fn bare_brackets() {
    case("[prod]", "Song [Ann x Bo]", "Song", &[("Ann", P), ("Bo", P)]);
    case("[feat]", "Song [Ann]", "Song", &[("Ann", F)]);
    case("[]", "Song [Dee]", "Song", &[("Dee", P)]);
    case("(prod)", "Song (Dee)", "Song", &[("Dee", P)]);
    case("[prod]", "Song (Dee)", "Song (Dee)", &[]);
    case("[prod]", "Song【Dee】", "Song", &[("Dee", P)]);
    // Words that are never names, version numbers.
    for title in ["Song [Official Audio]", "Song [v2]", "Song [2019]", "Song [Bo Remix]", "Song [Slowed + Reverb]", "Song [a cappella]"] {
        case("[prod]", title, title, &[]);
    }
    case("[prod] keep:-remix -mix", "Song [Bo Remix]", "Song", &[("Bo Remix", P)]);
    case("[prod] keep:\"bass boosted\"", "Song [Bass Boosted]", "Song [Bass Boosted]", &[]);
    // A keyword's bracket is the keyword rule's.
    case("[prod] prod", "Song [prod. Dee]", "Song", &[("Dee", P)]);
}

#[test]
fn prefix() {
    case("prefix", "Ann x Bo - Song", "Song", &[("Ann", M), ("Bo", M)]);
    case("prefix", "Ann – Song (Remix - Live)", "Song (Remix - Live)", &[("Ann", M)]);
    case("prefix", "Song (Remix - Live)", "Song (Remix - Live)", &[]);
    case("prefix", "Ann -", "Ann -", &[]);
    case("prefix feat", "Ann feat. Bo - Song", "Song", &[("Bo", F), ("Ann", M)]);
}

#[test]
fn the_order_typed_doesnt_matter() {
    assert_eq!(setting("[prod] prefix feat prod"), setting("prod feat [prod] prefix"));
}

#[test]
fn splitting() {
    case("feat", "Song (feat. Ann and Bo)", "Song", &[("Ann", F), ("Bo", F)]);
    case("feat", "Song (feat. Ann, Bo, and Cy)", "Song", &[("Ann", F), ("Bo", F), ("Cy", F)]);
    case("feat", "Song (feat. Ann,Bo)", "Song", &[("Ann", F), ("Bo", F)]);
    case("feat", "Song (feat. Ann&Bo)", "Song", &[("Ann&Bo", F)]);
    case("feat split:-x", "Song (feat. Ann x Bo)", "Song", &[("Ann x Bo", F)]);
    case("feat split:, &", "Song (feat. Ann x Bo and Cy, Dee)", "Song", &[("Ann x Bo and Cy", F), ("Dee", F)]);
    case("feat split:/", "Song (feat. Ann / Bo, Cy)", "Song", &[("Ann", F), ("Bo, Cy", F)]);
    // The same name twice in a role: once.
    case("feat", "Song (feat. Ann) [feat. ANN]", "Song", &[("Ann", F)]);
}

#[test]
fn keeping_whole() {
    let names = NameTable::new([(Uuid::from_u128(1), "Ann & The Bos"), (Uuid::from_u128(2), "Cy x Dee")]);
    let parsed = with_names("feat", "Song (feat. Eve, ANN & THE BOS x Fay)", &names);
    let got: Vec<(&str, bool)> = parsed.credits.iter().map(|c| (c.name.as_str(), c.whole)).collect();
    assert_eq!(got, [("Eve", false), ("ANN & THE BOS", true), ("Fay", false)]);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    let parsed = with_names("feat", "Song (feat. Cy x Dee x Eve)", &names);
    assert_eq!(parsed.credits.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Cy x Dee", "Eve"]);
    let parsed = with_names("feat whole:off", "Song (feat. Ann & The Bos)", &names);
    assert_eq!(parsed.credits.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Ann", "The Bos"]);
}

#[test]
fn fixed_by_hand() {
    let fixes = |not: &[&str], split: &[&str], whole: &[&str]| Fixes {
        not_names: not.iter().map(|s| s.to_string()).collect(),
        split: split.iter().map(|s| s.to_string()).collect(),
        whole: whole.iter().map(|s| s.to_string()).collect(),
    };
    let run = |rules: &str, title: &str, f: &Fixes, names: &NameTable| {
        let p = parse(title, &setting(rules), names, f);
        (p.title, p.credits.into_iter().map(|c| (c.name, c.whole)).collect::<Vec<_>>())
    };
    let none = NameTable::default();
    // Not a name: its part stays, nothing in it is credited.
    assert_eq!(run("feat [prod]", "Cool song! [bong] (feat. Ann)", &fixes(&["BONG"], &[], &[]), &none), ("Cool song! [bong]".into(), vec![("Ann".into(), false)]));
    assert_eq!(run("feat", "Song (feat. Ann & bong)", &fixes(&["bong"], &[], &[]), &none), ("Song (feat. Ann & bong)".into(), vec![]));
    // Kept whole, no artist needed.
    assert_eq!(run("feat", "Song (feat. Cy, Ann & The Bos)", &fixes(&[], &[], &["ann & the bos"]), &none), ("Song".into(), vec![("Cy".into(), false), ("Ann & The Bos".into(), true)]));
    // Split: an artist's name, and on separators not typed.
    let names = NameTable::new([(Uuid::from_u128(1), "Ann & The Bos")]);
    assert_eq!(run("feat", "Song (feat. Ann & The Bos)", &fixes(&[], &["Ann & The Bos"], &[]), &names).1, [("Ann".into(), false), ("The Bos".into(), false)]);
    assert_eq!(run("feat split:,", "Song (feat. Ann x Bo, Cy)", &fixes(&[], &["ann x bo"], &[]), &none).1, [("Ann".into(), false), ("Bo".into(), false), ("Cy".into(), false)]);
    // Where each came from.
    let p = parse("Song (feat. Ann, Bo) [prod. Dee]", &setting("feat prod"), &none, &Fixes::default());
    assert_eq!(p.credits.iter().map(|c| c.part.as_str()).collect::<Vec<_>>(), ["Dee", "Ann, Bo", "Ann, Bo"]);
}

#[test]
fn warnings() {
    let name = |name: &str, why| Warning::Name { name: name.into(), why };
    assert_eq!(warns("feat", "Song feat. Ann]"), [name("Ann]", "has a bracket in it")]);
    assert_eq!(warns("feat split:,", "Song (feat. Ann & Bo)"), [name("Ann & Bo", "might be more than one artist")]);
    assert_eq!(warns("[prod]", "Song [prod Dee]"), [name("prod Dee", "has a keyword in it")]);
    assert_eq!(warns("feat", "Song (feat. A)"), [name("A", "one character")]);
    assert_eq!(warns("feat", "(feat. Ann)"), [Warning::EmptyTitle]);
    assert_eq!(warns("prod", "Song (Remix [prod. Dee]"), [Warning::Brackets]);
    // A bracket left open inside another: read to the other's closing.
    case("feat", "Song (Remix [feat. Ann)", "Song (Remix)", &[("Ann", F)]);
    assert_eq!(warns("feat", "Song (Remix [feat. Ann)"), [Warning::Unclosed]);
    assert!(warns("feat prod [prod]", "Cool song!").is_empty());
}

#[test]
fn cutting_keeps_the_rest() {
    case("feat", "Song  (feat. Ann)  (Remix)", "Song (Remix)", &[("Ann", F)]);
    case("feat", "Song (feat. Ann)(Remix)", "Song (Remix)", &[("Ann", F)]);
    case("feat", "Song   two", "Song   two", &[]);
    case("prod", "Song & [prod. Dee]", "Song", &[("Dee", P)]);
}

#[test]
fn settings_read_back_the_same() {
    for text in [
        "feat prod [prod]",
        "with=prod +feat (feat) prefix",
        "feat split:-x -and",
        "feat split:, &",
        "feat whole:off",
        "feat keep:cdq2 \"bass boosted\" -live",
        "feat word:\"beat by\"=producer word:vs=featured",
    ] {
        let s = setting(text);
        assert_eq!(setting(&s.to_text()), s, "{text} → {}", s.to_text());
    }
    assert_eq!(setting("[prod] ft +f with=prod").to_text(), "feat with=producer +featured [producer]");
    assert_eq!(setting("feat keep:-live").to_text(), "feat keep:-live");
}

#[test]
fn settings_refused() {
    for (text, why) in [
        ("", "which rules"),
        ("nope", "no rule like “nope”"),
        ("+", "+ needs a role"),
        ("with=nobody", "no role like “nobody”"),
        ("feat ft", "there already"),
        ("[prod] [feat]", "there already"),
        ("word:vs", "needs a role"),
        ("feat split:, -x", "not both"),
        ("feat split:-y", "isn't split on by default"),
        ("feat keep:-nothing", "isn't one of the words"),
        ("feat whole:maybe", "on or off"),
    ] {
        let err = Setting::parse(text, &role).unwrap_err();
        assert!(err.contains(why), "{text}: {err}");
    }
}

#[test]
fn the_servers_check() {
    for title in ["Song (feat. Ann)", "Song ft Ann", "Song w/ Ann", "Song [prod. Dee]", "Song produced by Dee", "Song -feat Ann", "Song FEAT"] {
        assert!(server_flags(title), "{title}");
    }
    for title in ["Feathers", "Song w/o Ann", "Product", "Song (feat.Ann)", "Song with Ann", "Song x Ann"] {
        assert!(!server_flags(title), "{title}");
    }
    // Every keyword it flags, a rule finds.
    let ours: Vec<&str> = [Kind::Feat, Kind::With, Kind::Prod].iter().flat_map(|k| k.keywords()).collect();
    for k in SERVER_KEYWORDS {
        assert!(ours.contains(k), "{k}");
    }
}

/// `SERVER_REGEX` is the one in the newest migration that sets the check.
#[test]
fn the_servers_regex_is_the_same() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    let sql = files
        .iter()
        .rev()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .find(|sql| sql.contains("lacks || 'credits_in_title'"))
        .expect("a migration with the check");
    let line = sql.lines().find(|l| l.contains("t.title ~* '")).unwrap();
    let regex = line.split("~* '").nth(1).unwrap().split("' THEN").next().unwrap();
    assert_eq!(regex, SERVER_REGEX);
}
