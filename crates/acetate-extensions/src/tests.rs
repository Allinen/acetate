use super::*;

fn registry() -> Registry {
    Registry::of(vec![
        Manifest::parse(
            r#"
[extension]
name = "tube"
title = "Tube"
api = 3

[[shape]]
kind = "track"
id = "song_id"
fields = [{ name = "song_id", type = "str", required = true }]

[[shape]]
kind = "artist"
id = "channel"
fields = [{ name = "channel", type = "str", required = true }]

[id]
kind = "track"
field = "song_id"
class = "id"
len = 11
filename = true
bare = true

[[link]]
kind = "track"
open = "https://tube.example/watch?v={song_id}"
match = ["https://tube.example/watch?v={song_id}", "https://tu.be/{song_id}", "https://tube.example/shorts/{song_id}"]

[[link]]
kind = "artist"
match = ["https://tube.example/@{channel}", "https://tube.example/channel/{channel}"]
"#,
        )
        .unwrap(),
        Manifest::parse(
            r#"
[extension]
name = "cloud"
title = "Cloud"
api = 3

[[shape]]
kind = "track"
fields = [{ name = "song_permalinks", type = "strs" }]

[[shape]]
kind = "album"
fields = [{ name = "album_permalinks", type = "strs" }]

[[link]]
kind = "track"
comment = true
match = ["https://cloud.example/{user}/{track}"]
set = { song_permalinks = "{user}/{track}" }
captures = { track = { not = ["sets", "likes"] } }

[[link]]
kind = "album"
match = ["https://cloud.example/{user}/sets/{set}"]
set = { album_permalinks = "{user}/sets/{set}" }
"#,
        )
        .unwrap(),
        Manifest::parse(
            r#"
[extension]
name = "cn"
title = "CN"
api = 3

[[shape]]
kind = "track"
id = "song_id"
fields = [{ name = "song_id", type = "int", required = true }]

[[link]]
kind = "track"
match = ["https://music.cn/#/song?id={song_id}", "https://music.cn/song?id={song_id}"]
captures = { song_id = { class = "digits" } }
"#,
        )
        .unwrap(),
        Manifest::parse(
            r#"
[extension]
name = "stores"
title = "Stores"
api = 3

[[link]]
kind = "any"
custom = "shop"
comment = true
hosts = ["shop.example", "shop.page.link"]

[[link]]
kind = "any"
custom = "amazing"
comment = true
hosts = ["amazing.*"]
path_contains = "music"
"#,
        )
        .unwrap(),
    ])
}

fn found(r: &Registry, link: &str, kind: &str) -> Option<(String, Vec<(String, String)>)> {
    r.match_link(link, kind, false).map(|m| (m.platform, m.fields))
}

fn one(platform: &str, field: &str, value: &str) -> Option<(String, Vec<(String, String)>)> {
    Some((platform.into(), vec![(field.into(), value.into())]))
}

#[test]
fn links_are_read_by_their_templates() {
    let r = registry();
    assert_eq!(found(&r, "https://www.tube.example/watch?v=dQw4w9WgXcQ&t=1", "track"), one("tube", "song_id", "dQw4w9WgXcQ"));
    assert_eq!(found(&r, "tu.be/dQw4w9WgXcQ?si=x", "track"), one("tube", "song_id", "dQw4w9WgXcQ"));
    assert_eq!(found(&r, "https://m.tube.example/shorts/abc", "track"), one("tube", "song_id", "abc"));
    assert_eq!(found(&r, "https://tube.example/@someone", "artist"), one("tube", "channel", "someone"));
    assert_eq!(found(&r, "https://tube.example/channel/UCabc", "artist"), one("tube", "channel", "UCabc"));
    // The wrong kind, host or shape of path.
    assert_eq!(found(&r, "https://tube.example/watch?v=x", "artist"), None);
    assert_eq!(found(&r, "https://tube.example/watch", "track"), None);
    assert_eq!(found(&r, "https://other.example/watch?v=x", "track"), None);
    assert_eq!(found(&r, "https://tube.example/watch/more?v=x", "track"), None);
}

#[test]
fn parts_make_other_fields_and_can_be_refused() {
    let r = registry();
    assert_eq!(found(&r, "https://cloud.example/a/b?utm=1", "track"), one("cloud", "song_permalinks", "a/b"));
    assert_eq!(found(&r, "https://cloud.example/a/sets", "track"), None);
    assert_eq!(found(&r, "https://cloud.example/a/likes", "track"), None);
    assert_eq!(found(&r, "https://cloud.example/a/sets/tape", "album"), one("cloud", "album_permalinks", "a/sets/tape"));
    // Only links that count in a comment.
    assert!(r.match_link("https://tube.example/watch?v=x", "track", true).is_none());
    assert!(r.match_link("https://cloud.example/a/b", "track", true).is_some());
}

#[test]
fn fragments_and_classes() {
    let r = registry();
    assert_eq!(found(&r, "https://music.cn/#/song?id=123", "track"), one("cn", "song_id", "123"));
    assert_eq!(found(&r, "https://music.cn/song?id=123", "track"), one("cn", "song_id", "123"));
    assert_eq!(found(&r, "https://music.cn/#/song?id=abc", "track"), None);
    assert_eq!(found(&r, "https://music.cn/#/album?id=1", "track"), None);
}

#[test]
fn stores_are_custom_sources() {
    let r = registry();
    let shop = |url: &str| Some(("custom".to_string(), vec![("label".to_string(), "shop".to_string()), ("url".to_string(), url.to_string())]));
    assert_eq!(found(&r, "https://open.shop.example/track/1", "track"), shop("https://open.shop.example/track/1"));
    assert_eq!(found(&r, "shop.page.link/x", "album"), shop("https://shop.page.link/x"));
    assert_eq!(found(&r, "https://notshop.example/x", "track"), None);
    assert!(found(&r, "https://amazing.com/music/albums/1", "track").is_some());
    assert!(found(&r, "https://amazing.com/books/1", "track").is_none());
}

#[test]
fn ids_pages_and_data() {
    let r = registry();
    assert!(r.id_rule("tube").unwrap().is_id("dQw4w9WgXcQ"));
    assert!(r.id_rule("tube").unwrap().is_id("helloworld1"));
    assert!(!r.id_rule("tube").unwrap().is_id("short"));
    assert_eq!(r.match_bare_id(" dQw4w9WgXcQ ", "track").map(|m| m.platform), Some("tube".to_string()));
    assert_eq!(r.open_url("tube", "track", &serde_json::json!({ "song_id": "abc" })).as_deref(), Some("https://tube.example/watch?v=abc"));
    assert_eq!(r.open_url("tube", "track", &serde_json::json!({})), None);
    assert_eq!(r.data_from_fields("cloud", "track", &[("song_permalinks".into(), "a/b".into())]).unwrap(), serde_json::json!({ "song_permalinks": ["a/b"] }));
    assert!(r.data_from_fields("cn", "track", &[("song_id".into(), "x".into())]).is_err());
}

#[test]
fn a_badly_made_manifest_says_why() {
    let bad = |extra: &str| Manifest::parse(&format!("[extension]\nname = \"x\"\ntitle = \"X\"\napi = 3\n[[shape]]\nkind = \"track\"\nfields = [{{ name = \"a\", type = \"str\" }}]\n{extra}")).unwrap_err();
    assert!(bad("[[link]]\nkind = \"track\"\nmatch = [\"https://x.example/{b}\"]\n").contains("no field"));
    assert!(bad("[[link]]\nkind = \"album\"\nmatch = [\"https://x.example/{a}\"]\n").contains("no album shape"));
    assert!(bad("[[link]]\nkind = \"track\"\n").contains("something to match"));
    assert!(Manifest::parse("[extension]\nname = \"X y\"\ntitle = \"X\"\napi = 3\n").is_err());
    assert!(Manifest::parse("[extension]\nname = \"x\"\ntitle = \"X\"\napi = 3\nsurprise = 1\n").is_err());
}

#[test]
fn a_folder_adds_and_replaces_and_says_what_it_skipped() {
    let dir = std::env::temp_dir().join(format!("acetate-extensions-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let write = |name: &str, text: &str| {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("extension.toml"), text).unwrap();
    };
    write("beats", "[extension]\nname = \"beats\"\ntitle = \"Beats\"\napi = 1\n[[shape]]\nkind = \"track\"\nid = \"track_id\"\nfields = [{ name = \"track_id\", type = \"int\", required = true }]\n");
    write("youtube", "[extension]\nname = \"youtube\"\ntitle = \"My YouTube\"\napi = 1\n");
    write("future", &format!("[extension]\nname = \"future\"\ntitle = \"F\"\napi = {}\n", API + 1));
    write("broken", "[extension]\nname = \"broken\"\n");
    let (registry, problems) = Registry::with_folder(&dir);
    assert!(registry.shape("beats", "track").is_some());
    assert!(registry.shape("youtube", "track").is_none(), "replaced by the folder's");
    assert!(registry.shape("soundcloud", "track").is_some());
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("needs API")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_built_in_ones_load() {
    let r = Registry::builtin();
    assert!(r.shape("youtube", "track").is_some() && r.shape("custom", "artist").is_some());
}

#[test]
fn the_built_in_ones_read_the_links_they_always_did() {
    let r = Registry::builtin();
    let id = |link: &str| r.match_bare_id(link, "track").or_else(|| r.match_link(link, "track", false)).map(|m| (m.platform, m.fields));
    let youtube = Some(("youtube".to_string(), vec![("song_id".to_string(), "dQw4w9WgXcQ".to_string())]));
    for link in [
        "dQw4w9WgXcQ",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
        "https://music.youtube.com/watch?list=x&v=dQw4w9WgXcQ",
        "http://youtu.be/dQw4w9WgXcQ?si=abc",
        "youtube.com/shorts/dQw4w9WgXcQ",
        "https://m.youtube.com/embed/dQw4w9WgXcQ",
    ] {
        assert_eq!(id(link), youtube, "{link}");
    }
    assert_eq!(id("https://evil.example/watch?v=dQw4w9WgXcQ"), None);
    assert_eq!(id("https://www.youtube.com/watch?v=short"), None);
    assert_eq!(found(r, "https://www.youtube.com/@channel", "artist"), one("youtube", "channel", "channel"));
    assert_eq!(found(r, "https://soundcloud.com/qlvarn/sets/tape", "album"), one("soundcloud", "album_permalinks", "qlvarn/sets/tape"));
    assert_eq!(found(r, "https://m.soundcloud.com/qlvarn", "artist"), one("soundcloud", "artist_permalinks", "qlvarn"));
    assert_eq!(found(r, "https://music.163.com/#/song?id=123", "track"), one("netease", "song_id", "123"));
    assert_eq!(found(r, "https://music.163.com/#/song?id=1", "album"), None);
    assert!(r.reads_site_of("https://music.163.com/#/song?id=1") && !r.reads_site_of("https://drive.google.com/x"));
}
