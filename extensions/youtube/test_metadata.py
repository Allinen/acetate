"""`python3 -m unittest` in this folder."""

import unittest

import metadata

VIDEO = {
    "id": "dQw4w9WgXcQ", "title": "Mira Vale - Glass Harbor (Official Video)", "extractor_key": "Youtube",
    "channel": "Mira Vale", "channel_id": "UCabc", "uploader_id": "@miravale",
    "upload_date": "20210604", "description": "", "thumbnail": "https://i.ytimg.com/vi/x/maxresdefault.jpg",
}


class Normalize(unittest.TestCase):
    def test_a_video(self):
        m = metadata.normalize(VIDEO)
        self.assertEqual((m["from"], m["title"], m["date"]), ("YouTube", "Mira Vale - Glass Harbor (Official Video)", "2021-06-04"))
        self.assertEqual(m["artists"], ["Mira Vale"])
        self.assertEqual(m["source"], {"platform": "youtube", "id": "dQw4w9WgXcQ", "data": {"song_id": "dQw4w9WgXcQ", "song_release_date": "2021-06-04"}})
        self.assertEqual(m["account"], {"platform": "youtube", "ids": ["UCabc", "miravale"], "label": "youtube @miravale", "data": {"channel": "UCabc"}})
        self.assertEqual(m["artwork"], ["https://i.ytimg.com/vi/x/maxresdefault.jpg"])

    def test_youtube_musics_fields_are_the_artists_with_no_account(self):
        music = dict(VIDEO, artists=["Mira Vale", "Bo"], track="Glass Harbor", release_date="20210101")
        m = metadata.normalize(music)
        self.assertEqual((m["title"], len(m["artists"]), m["account"], m["date"]), ("Glass Harbor", 2, None, "2021-01-01"))

    def test_what_is_missing_is_said(self):
        for key, why in [("id", "video id"), ("title", "title")]:
            with self.assertRaisesRegex(ValueError, why):
                metadata.normalize({k: v for k, v in VIDEO.items() if k != key})
        with self.assertRaisesRegex(ValueError, "no artist"):
            metadata.normalize({"id": "x", "title": "t"})

    def test_pasted_json_of_another_site_is_refused(self):
        with self.assertRaisesRegex(ValueError, "only YouTube"):
            metadata.answer({"op": "parse", "input": {"id": "1", "extractor_key": "Soundcloud", "title": "x"}})
        self.assertEqual(metadata.answer({"op": "parse", "input": VIDEO})["source"]["id"], "dQw4w9WgXcQ")


if __name__ == "__main__":
    unittest.main()
