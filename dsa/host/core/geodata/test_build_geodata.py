"""Tests for build-geodata.py: how Korea's dongs become address parts, how boundaries are
simplified, and the file format, which dsa_host_core::boundaries reads.

tests/data/address-boundaries-fixture.bin in the host core is written by `fixture()` below.
The Rust tests read that file, and test_fr36_the_fixture_is_what_the_writer_makes keeps it the
writer's, so the two sides cannot drift apart. After changing the format, write it again:

    uv run dsa/host/core/geodata/test_build_geodata.py --write-fixture
"""

import importlib.util
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent / "tests" / "data" / "address-boundaries-fixture.bin"

_spec = importlib.util.spec_from_file_location("build_geodata", HERE / "build-geodata.py")
geodata = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(geodata)


def square(lat, lon, half):
    """A square ring around (lat, lon), in file units."""
    s = geodata.SCALE
    return [(round((lat - half) * s), round((lon - half) * s)), (round((lat - half) * s), round((lon + half) * s)),
            (round((lat + half) * s), round((lon + half) * s)), (round((lat + half) * s), round((lon - half) * s))]


def fixture():
    """A small file: two dongs in Seoul and Suwon, a province around them, Tokyo's
    prefecture with a hole in it, and three cities."""
    w = geodata.Writer("fixture")
    kr = w.country("KR", w.name("South Korea", {"ko": "대한민국", "ja": "大韓民国"}))
    jp = w.country("JP", w.name("Japan", {"ko": "일본", "ja": "日本"}))

    def korean(adm_nm, sido):
        parts = geodata.korean_parts(adm_nm, sido)
        return [w.name(parts[field]) if parts[field] else None for field in geodata.FIELDS]

    korea = [
        (kr, korean("서울특별시 중구 명동", "서울특별시"), [square(37.5663, 126.9779, 0.005)]),
        (kr, korean("경기도 수원시장안구 조원2동", "경기도"), [square(37.3036, 127.0102, 0.005)]),
    ]
    world = [
        (kr, [w.name("Seoul", {"ko": "서울특별시", "ja": "ソウル特別市"}), None, None, None, None],
         [square(37.55, 126.95, 0.3)]),
        (jp, [w.name("Tokyo", {"ko": "도쿄도", "ja": "東京都"}), None, None, None, None],
         [square(35.68, 139.76, 0.5), square(35.68, 139.76, 0.05)]),
    ]
    w.layers = [korea, world]
    w.places = [
        (kr, w.name("Seoul", {"ko": "서울", "ja": "ソウル"}), round(37.5665 * geodata.SCALE), round(126.978 * geodata.SCALE)),
        (jp, w.name("Tokyo", {"ko": "도쿄", "ja": "東京"}), round(35.6895 * geodata.SCALE), round(139.6917 * geodata.SCALE)),
        (jp, w.name("Hachioji", {"ja": "八王子"}), round(35.6664 * geodata.SCALE), round(139.316 * geodata.SCALE)),
    ]
    return w.pack()


class KoreanPartsTests(unittest.TestCase):
    def test_fr36_a_metropolitan_dong_is_city_district_and_dong(self):
        self.assertEqual(geodata.korean_parts("서울특별시 종로구 사직동", "서울특별시"),
                         {"admin_area": "서울특별시", "sub_admin_area": "", "locality": "서울특별시",
                          "sub_locality": "종로구", "thoroughfare": "사직동"})

    def test_fr36_a_city_with_districts_written_as_one_word_is_split(self):
        parts = geodata.korean_parts("경기도 수원시장안구 파장동", "경기도")
        self.assertEqual((parts["locality"], parts["sub_locality"], parts["thoroughfare"]),
                         ("수원시", "장안구", "파장동"))
        parts = geodata.korean_parts("경상북도 포항시남구 대이동", "경상북도")
        self.assertEqual((parts["locality"], parts["sub_locality"]), ("포항시", "남구"))

    def test_fr36_a_county_town_has_no_district(self):
        parts = geodata.korean_parts("경기도 양평군 양평읍", "경기도")
        self.assertEqual((parts["admin_area"], parts["locality"], parts["sub_locality"], parts["thoroughfare"]),
                         ("경기도", "양평군", "", "양평읍"))
        # A city without districts, whose name has 시 in it, stays one word.
        parts = geodata.korean_parts("경기도 시흥시 신천동", "경기도")
        self.assertEqual((parts["locality"], parts["sub_locality"]), ("시흥시", ""))

    def test_fr36_sejong_has_no_district(self):
        parts = geodata.korean_parts("세종특별자치시 조치원읍", "세종특별자치시")
        self.assertEqual((parts["locality"], parts["sub_locality"], parts["thoroughfare"]),
                         ("세종특별자치시", "", "조치원읍"))


class SimplifyTests(unittest.TestCase):
    def test_fr36_points_on_a_straight_edge_are_dropped_and_corners_kept(self):
        ring = [(0, 0), (0, 50), (0, 100), (100, 100), (100, 0)]
        self.assertEqual(geodata.simplify(ring, 5), [(0, 0), (0, 100), (100, 100), (100, 0)])

    def test_fr36_a_ring_smaller_than_the_tolerance_goes(self):
        self.assertEqual(geodata.simplify([(0, 0), (0, 1), (1, 1), (1, 0)], 5), [])

    def test_fr36_rings_are_quantized_and_left_open(self):
        ring = [(126.97790, 37.56630), (126.97791, 37.56631), (127.0, 37.6), (126.9779, 37.5663)]
        self.assertEqual(geodata.quantized(ring), [(3756630, 12697790), (3756631, 12697791), (3760000, 12700000)])


class FormatTests(unittest.TestCase):
    def test_fr36_the_fixture_is_what_the_writer_makes(self):
        self.assertEqual(FIXTURE.read_bytes(), fixture(),
                         "the fixture is stale: write it again with --write-fixture")

    def test_fr36_a_written_file_reads_back(self):
        decoded = geodata.decode(fixture())
        self.assertEqual(decoded["made_from"], "fixture")
        self.assertEqual([code for code, _ in decoded["countries"]], ["KR", "JP"])
        region = geodata.lookup(decoded, 37.3036, 127.0102)
        self.assertEqual(region["locality"], {"": "수원시"})
        self.assertEqual(region["thoroughfare"], {"": "조원2동"})
        # Inside Tokyo's hole: in no region.
        self.assertIsNone(geodata.lookup(decoded, 35.68, 139.76))
        self.assertEqual(geodata.lookup(decoded, 35.9, 139.9)["admin_area"]["ja"], "東京都")

    def test_fr36_negative_and_large_numbers_survive(self):
        out = bytearray()
        for value in (0, 1, -1, 18_000_000, -18_000_000):
            geodata.put_zigzag(out, value)
        r = geodata.Reader(bytes(out))
        self.assertEqual([r.zigzag() for _ in range(5)], [0, 1, -1, 18_000_000, -18_000_000])

    def test_fr36_the_checks_catch_a_wrong_city(self):
        w = geodata.Writer("wrong")
        kr = w.country("KR", w.name("South Korea"))
        w.layers = [[(kr, [w.name("부산광역시"), None, w.name("부산광역시"), w.name("중구"), w.name("중앙동")],
                      [square(37.5663, 126.9779, 0.01)])]]
        problems = geodata.check(geodata.decode(w.pack()))
        self.assertTrue(any("서울특별시" in problem for problem in problems), problems)

    def test_fr36_every_source_is_pinned(self):
        for name, source in geodata.SOURCES.items():
            self.assertRegex(source["sha256"], r"^[0-9a-f]{64}$", name)
            # A commit, not a branch.
            self.assertRegex(source["url"], r"/[0-9a-f]{40}/", name)


if __name__ == "__main__":
    if "--write-fixture" in sys.argv:
        FIXTURE.parent.mkdir(parents=True, exist_ok=True)
        FIXTURE.write_bytes(fixture())
        print(f"wrote {FIXTURE}")
    else:
        unittest.main()
