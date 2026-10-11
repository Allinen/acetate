"""`python3 -m unittest` in this folder."""

import unittest

import metadata


def track():
    return {
        "kind": "track", "id": 5550123, "title": "Glass Harbor (feat. Bo)",
        "permalink_url": "https://soundcloud.com/miravale/glass-harbor",
        "waveform_url": "https://wave.sndcdn.com/AbCdEf123_m.json",
        "created_at": "2021-06-04T11:11:11Z", "display_date": "2021-06-05T00:00:00Z",
        "description": "out now", "genre": "Electronic",
        "artwork_url": "https://i1.sndcdn.com/artworks-xyz-large.jpg",
        "user": {"id": 42, "username": "Mira Vale", "permalink": "miravale"},
        "publisher_metadata": {"artist": ""},
    }


class Parse(unittest.TestCase):
    def test_a_track(self):
        m = metadata.parse(track())
        self.assertEqual((m["from"], m["title"], m["date"], m["genre"]), ("SoundCloud", "Glass Harbor (feat. Bo)", "2021-06-05T00:00:00Z", "Electronic"))
        self.assertEqual(m["artists"], ["Mira Vale"])
        self.assertEqual(m["source"], {"platform": "soundcloud", "id": "5550123", "data": {
            "song_id": 5550123, "song_uids": ["AbCdEf123"], "song_permalinks": ["miravale/glass-harbor"],
            "release_dates": ["2021-06-05T00:00:00Z"], "created_at": ["2021-06-04T11:11:11Z"], "song_descriptions": ["out now"]}})
        self.assertEqual((m["account"]["ids"], m["account"]["data"]), (["42"], {"artist_id": 42, "artist_permalinks": ["miravale"]}))
        self.assertEqual(m["artwork"], ["https://i1.sndcdn.com/artworks-xyz-original.jpg", "https://i1.sndcdn.com/artworks-xyz-original.png", "https://i1.sndcdn.com/artworks-xyz-large.jpg"])
        self.assertEqual(metadata.artwork_links("https://i1.sndcdn.com/artworks-000123-AbC-t500x500.png?x=1"), [
            "https://i1.sndcdn.com/artworks-000123-AbC-original.jpg", "https://i1.sndcdn.com/artworks-000123-AbC-original.png",
            "https://i1.sndcdn.com/artworks-000123-AbC-t500x500.png?x=1"])

    def test_details(self):
        # The publisher's artist, not the uploader (a label): no account.
        label = track()
        label["publisher_metadata"]["artist"] = "Ann"
        m = metadata.parse(label)
        self.assertEqual((m["artists"], m["account"]), (["Ann"], None))
        # The waveform from `waveformKey`; no display date: created_at.
        keyed = track()
        del keyed["waveform_url"], keyed["display_date"]
        keyed["waveformKey"] = "XyZ_m"
        m = metadata.parse(keyed)
        self.assertEqual((m["source"]["data"]["song_uids"], m["date"]), (["XyZ"], "2021-06-04T11:11:11Z"))
        # No waveform: refused.
        null = track()
        null["waveform_url"] = None
        with self.assertRaisesRegex(ValueError, "waveform_url is null"):
            metadata.parse(null)
        del keyed["waveformKey"]
        with self.assertRaises(ValueError):
            metadata.parse(keyed)


if __name__ == "__main__":
    unittest.main()
