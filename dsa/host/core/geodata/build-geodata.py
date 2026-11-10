"""Builds the boundary data the host turns positions into addresses with.

Android's Geocoder asks the host for the address of a position. The host answers from this
file alone: no server, no key, and the position never leaves the computer. The file holds
administrative boundaries, simplified and packed:

  - Korea at neighbourhood level (읍면동): Statistics Korea's administrative-dong boundaries
    (SGIS), as vuski/admdongkor keeps them with their changes over time. Each dong carries
    its province or metropolitan city (시도), its city, county or district (시군구) and its own
    name.
  - The world at province level: Natural Earth's admin-1 states and provinces, with their
    names in Natural Earth's languages, and the country of each.
  - The world's cities and towns as points: Natural Earth's populated places. Outside the
    detailed boundaries the host names the nearest one in the same country as the city.

Every source is fetched at a pinned revision and checked against its SHA-256 before it is
read, so the same script always makes the same file. Downloads are kept in the cache folder
(by default .scratch/geodata/ in the repository) and reused.

    build-geodata.py [--cache <folder>] --out <file>

It writes NOTICE.txt beside the file, with the attribution the sources' licences ask for:
CC BY 4.0 and KOGL Type 1 for the Korean boundaries; Natural Earth is in the public domain.
The packaging scripts run it and install both files beside the host programs: in geodata/
under the install folder on Windows, /usr/lib/dsa/geodata/ on Linux and
DSA.app/Contents/Resources/geodata/ on macOS.

The format (little endian; "varint" is LEB128, "zigzag" a signed varint):

    b"DSAGEO01"
    varint len, UTF-8         what the file was made from, for the host's log
    varint count, then each   a language tag ("" first: the name a place has without one)
    varint count, then each   a string: varint len, UTF-8
    varint count, then each   a name: varint entries, each varint language and varint string
    varint count, then each   a country: 2 ASCII bytes (ISO 3166-1 alpha-2), varint name
    varint count, then each   a layer, the most detailed first:
        varint count, then each region:
            varint country + 1 (0: none)
            5 x varint name + 1 (0: none): admin area, sub-admin area, locality,
                                            sub-locality, thoroughfare
            varint rings, then each ring: varint points, then the points as zigzag
            latitude and longitude in units of 1e-5 degrees, the first absolute and every
            other one the difference from the one before
    varint count, then each   a place: varint country + 1, varint name, zigzag latitude and
                              zigzag longitude (1e-5 degrees)

A point is inside a region when it is inside an odd number of the region's rings, so holes
and islands need nothing of their own. dsa_host_core::boundaries reads the file.
"""

import argparse
import hashlib
import json
import re
import sys
import urllib.request
from pathlib import Path

MAGIC = b"DSAGEO01"
NOTICE_NAME = "NOTICE.txt"
# 1e-5 degrees is about 1.1 m: far finer than the boundaries are simplified to.
SCALE = 100_000
OUTPUT_NAME = "address-boundaries.bin"

_GH = "https://raw.githubusercontent.com"
_NE = f"{_GH}/nvkelso/natural-earth-vector/ca96624a56bd078437bca8184e78163e5039ad19/geojson"
SOURCES = {
    "korea": {
        "url": f"{_GH}/vuski/admdongkor/d5d5a821c8de6cc337d47eeca604d7af76271c9b/"
               "ver20261001/HangJeongDong_ver20261001.geojson",
        "sha256": "843ece3a0b3429742cd4a1b699cc13987a88b5e5401f382e7fdaaa194ea5e605",
        "what": "admdongkor ver20261001 (Statistics Korea SGIS administrative dongs)",
        "notice": ("Korean administrative dong boundaries: this data is derived from administrative-dong "
                   "boundaries released by Statistics Korea SGIS (https://sgis.kostat.go.kr) under KOGL "
                   "Type 1 (https://www.kogl.or.kr/info/licenseType1.do), modified by vuski/admdongkor "
                   "(https://github.com/vuski/admdongkor), distributed under CC BY 4.0 "
                   "(https://creativecommons.org/licenses/by/4.0/). DSA simplified the boundaries and "
                   "keeps only their names.\n"
                   "본 데이터는 통계청 통계지리정보서비스(SGIS, https://sgis.kostat.go.kr)에서 공공누리 제1유형으로 "
                   "개방한 행정동 경계를 가공한 것이며(가공: vuski/admdongkor, https://github.com/vuski/admdongkor), "
                   "CC BY 4.0으로 배포됩니다. DSA가 경계를 단순화하고 이름만 남겼습니다."),
    },
    "admin1": {
        "url": f"{_NE}/ne_10m_admin_1_states_provinces.geojson",
        "sha256": "22d0e3ad85eb3e27f17cabf8ba2d50e554fbc27a87796ff891d958185da62fb5",
        "what": "Natural Earth 5.1.2 admin-1 states and provinces (1:10m)",
    },
    "admin0": {
        "url": f"{_NE}/ne_10m_admin_0_countries.geojson",
        "sha256": "239eec57ac17f100a11e2536cffc56752c318b50ae765b0918ff7aab4ce8f255",
        "what": "Natural Earth 5.1.2 countries (1:10m), for the countries' names",
    },
    "places": {
        "url": f"{_NE}/ne_10m_populated_places.geojson",
        "sha256": "9b8e3de09048ef00dfc70357dbb9fa324493f214b5e0ae4daf1aa79a8d10116b",
        "what": "Natural Earth 5.1.2 populated places (1:10m)",
        "notice": ("States and provinces, countries and populated places: made with Natural Earth. Free "
                   "vector and raster map data @ naturalearthdata.com. Natural Earth is in the public "
                   "domain (https://www.naturalearthdata.com/about/terms-of-use/)."),
    },
}

# How far a boundary may move when it is simplified, in units of 1e-5 degrees: about 15 m
# for Korea's dongs, about 1 km for the world's provinces.
KOREA_TOLERANCE = 15
WORLD_TOLERANCE = 1000

# Natural Earth's name languages (name_<tag>); "zht" is Traditional Chinese.
LANGUAGES = ["", "ar", "bn", "de", "el", "en", "es", "fa", "fr", "he", "hi", "hu", "id", "it",
             "ja", "ko", "nl", "pl", "pt", "ru", "sv", "tr", "uk", "ur", "vi", "zh", "zht"]

# Korea's metropolitan cities: a dong there belongs to the city itself, and the next level
# down is a district (구) or a county (군).
METROPOLITAN_SUFFIXES = ("특별시", "광역시", "특별자치시")

# Known positions and what the file must say of them, checked after every build.
CHECKS = [
    # Seoul City Hall
    ((37.5663, 126.9779), {"country": "KR", "admin_area": "서울특별시", "locality": "서울특별시",
                           "sub_locality": "중구"}),
    # Suwon, Jangan-gu
    ((37.3036, 127.0102), {"country": "KR", "admin_area": "경기도", "locality": "수원시",
                           "sub_locality": "장안구"}),
    # Paris
    ((48.8566, 2.3522), {"country": "FR"}),
    # Manhattan
    ((40.7831, -73.9712), {"country": "US", "admin_area": "New York"}),
    # Tokyo Station
    ((35.6812, 139.7671), {"country": "JP"}),
]


def fetch(name, cache):
    """The source `name`, downloaded into `cache` unless it is there with the right hash."""
    source = SOURCES[name]
    path = Path(cache) / f"{name}-{source['sha256'][:12]}.geojson"
    if not path.exists() or sha256_of(path) != source["sha256"]:
        path.parent.mkdir(parents=True, exist_ok=True)
        partial = path.with_suffix(".part")
        request = urllib.request.Request(source["url"], headers={"User-Agent": "dsa-build-geodata"})
        with urllib.request.urlopen(request, timeout=300) as response, partial.open("wb") as out:
            while chunk := response.read(1 << 20):
                out.write(chunk)
        digest = sha256_of(partial)
        if digest != source["sha256"]:
            partial.unlink()
            raise SystemExit(f"{source['url']} has SHA-256 {digest}, not the pinned {source['sha256']}")
        partial.replace(path)
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_of(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as f:
        while chunk := f.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def rings_of(geometry):
    """Every ring of a Polygon or MultiPolygon, as (longitude, latitude) lists."""
    if not geometry:
        return []
    if geometry["type"] == "Polygon":
        return list(geometry["coordinates"])
    if geometry["type"] == "MultiPolygon":
        return [ring for polygon in geometry["coordinates"] for ring in polygon]
    return []


def quantized(ring):
    """A ring as (latitude, longitude) in units of 1e-5 degrees, repeated points dropped and
    the ring left open (its last point is not its first)."""
    points = []
    for lon, lat, *_ in ring:
        point = (round(lat * SCALE), round(lon * SCALE))
        if not points or points[-1] != point:
            points.append(point)
    if len(points) > 1 and points[0] == points[-1]:
        points.pop()
    return points


def simplify(points, tolerance):
    """The open ring `points` with every point dropped that lies within `tolerance` of the
    line its kept neighbours draw (Douglas-Peucker), or [] when fewer than three are left."""
    if len(points) < 3:
        return []
    # Split the ring at its first point and the point farthest from it, and simplify the two
    # halves as lines.
    first = points[0]
    far = max(range(len(points)), key=lambda i: (points[i][0] - first[0]) ** 2 + (points[i][1] - first[1]) ** 2)
    if far == 0:
        return []
    closed = points + [first]
    keep = [False] * len(closed)
    keep[0] = keep[far] = keep[-1] = True
    stack = [(0, far), (far, len(closed) - 1)]
    limit = tolerance * tolerance
    while stack:
        start, end = stack.pop()
        if end - start < 2:
            continue
        (y1, x1), (y2, x2) = closed[start], closed[end]
        dy, dx = y2 - y1, x2 - x1
        length = dx * dx + dy * dy
        best, best_distance = -1, -1.0
        for i in range(start + 1, end):
            y, x = closed[i]
            if length == 0:
                distance = (x - x1) ** 2 + (y - y1) ** 2
            else:
                cross = dx * (y - y1) - dy * (x - x1)
                distance = cross * cross / length
            if distance > best_distance:
                best, best_distance = i, distance
        if best_distance > limit:
            keep[best] = True
            stack.append((start, best))
            stack.append((best, end))
    kept = [point for point, k in zip(closed[:-1], keep[:-1]) if k]
    return kept if len(kept) >= 3 else []


def korean_parts(adm_nm, sido):
    """The address parts of a Korean dong from its full name ("경기도 수원시 장안구 파장동")
    and its province or metropolitan city: the admin area is the 시도, the locality the city
    (the metropolitan city itself, or the 시 or 군 of a province), the sub-locality the
    district (구) or a metropolitan city's county (군), and the thoroughfare the dong."""
    words = adm_nm.split()
    if len(words) < 2:
        raise ValueError(f"the dong {adm_nm!r} has no name of its own")
    sido = sido or words[0]
    middle, dong = [], words[-1]
    for word in words[1:-1]:
        # A city with districts is written as one word ("수원시장안구"): split it.
        match = re.fullmatch(r"(.+?시)(.+구)", word)
        middle += [match.group(1), match.group(2)] if match else [word]
    if sido.endswith(METROPOLITAN_SUFFIXES):
        locality = sido
        sub_locality = " ".join(middle)
    else:
        locality = middle[0] if middle else ""
        sub_locality = " ".join(middle[1:])
    return {"admin_area": sido, "sub_admin_area": "", "locality": locality,
            "sub_locality": sub_locality, "thoroughfare": dong}


FIELDS = ["admin_area", "sub_admin_area", "locality", "sub_locality", "thoroughfare"]


class Writer:
    """Collects strings, names, countries, layers and places, and packs them."""

    def __init__(self, made_from):
        self.made_from = made_from
        self.strings, self.string_ids = [], {}
        self.names, self.name_ids = [], {}
        self.countries, self.country_ids = [], {}
        self.layers = []
        self.places = []

    def string(self, text):
        if text not in self.string_ids:
            self.string_ids[text] = len(self.strings)
            self.strings.append(text)
        return self.string_ids[text]

    def name(self, default, translations=None):
        """A name: its own text, and the languages that call it something else."""
        entries = [(0, self.string(default))]
        for language, text in sorted((translations or {}).items()):
            if text and text != default:
                entries.append((LANGUAGES.index(language), self.string(text)))
        key = tuple(entries)
        if key not in self.name_ids:
            self.name_ids[key] = len(self.names)
            self.names.append(key)
        return self.name_ids[key]

    def country(self, code, name):
        if code not in self.country_ids:
            self.country_ids[code] = len(self.countries)
            self.countries.append((code, name))
        return self.country_ids[code]

    def pack(self):
        out = bytearray(MAGIC)
        put_text(out, self.made_from)
        put_varint(out, len(LANGUAGES))
        for language in LANGUAGES:
            put_text(out, language)
        put_varint(out, len(self.strings))
        for text in self.strings:
            put_text(out, text)
        put_varint(out, len(self.names))
        for entries in self.names:
            put_varint(out, len(entries))
            for language, string in entries:
                put_varint(out, language)
                put_varint(out, string)
        put_varint(out, len(self.countries))
        for code, name in self.countries:
            out += code.encode("ascii")
            put_varint(out, name)
        put_varint(out, len(self.layers))
        for regions in self.layers:
            put_varint(out, len(regions))
            for country, fields, rings in regions:
                put_varint(out, 0 if country is None else country + 1)
                for field in fields:
                    put_varint(out, 0 if field is None else field + 1)
                put_varint(out, len(rings))
                for ring in rings:
                    put_varint(out, len(ring))
                    last = (0, 0)
                    for lat, lon in ring:
                        put_zigzag(out, lat - last[0])
                        put_zigzag(out, lon - last[1])
                        last = (lat, lon)
        put_varint(out, len(self.places))
        for country, name, lat, lon in self.places:
            put_varint(out, 0 if country is None else country + 1)
            put_varint(out, name)
            put_zigzag(out, lat)
            put_zigzag(out, lon)
        return bytes(out)


def put_varint(out, value):
    if value < 0:
        raise ValueError(f"{value} is negative")
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return


def put_zigzag(out, value):
    put_varint(out, (value << 1) if value >= 0 else ((-value << 1) - 1))


def put_text(out, text):
    data = text.encode("utf-8")
    put_varint(out, len(data))
    out += data


def translations(props, upper=False):
    """The names of a Natural Earth feature by language (name_ko, or NAME_KO)."""
    found = {}
    for language in LANGUAGES[1:]:
        key = f"NAME_{language.upper()}" if upper else f"name_{language}"
        value = props.get(key)
        if isinstance(value, str) and value.strip():
            found[language] = value.strip()
    return found


def country_code(props):
    """A Natural Earth country's ISO 3166-1 alpha-2 code, or None (some have -99)."""
    for key in ("ISO_A2_EH", "ISO_A2", "iso_a2"):
        code = props.get(key)
        if isinstance(code, str) and len(code) == 2 and code.isalpha():
            return code.upper()
    return None


def build(sources):
    """The packed file from the parsed sources ({name: GeoJSON}), and what it holds."""
    writer = Writer("; ".join(source["what"] for source in SOURCES.values()))

    countries = {}
    by_a3 = {}
    for feature in sources["admin0"]["features"]:
        props = feature["properties"]
        code = country_code(props)
        if code is None:
            continue
        by_a3[props.get("ADM0_A3")] = code
        if code not in countries:
            countries[code] = writer.country(code, writer.name(props["NAME"], translations(props, upper=True)))

    def country_of(code, a3):
        code = code if isinstance(code, str) and len(code) == 2 and code.isalpha() else by_a3.get(a3)
        return countries.get(code.upper()) if code else None

    stats = {"points_before": 0, "points_after": 0}

    def rings(geometry, tolerance):
        kept = []
        for ring in rings_of(geometry):
            points = quantized(ring)
            stats["points_before"] += len(points)
            simplified = simplify(points, tolerance)
            if simplified:
                stats["points_after"] += len(simplified)
                kept.append(simplified)
        return kept

    korea = []
    korea_country = countries.get("KR")
    for feature in sources["korea"]["features"]:
        props = feature["properties"]
        parts = korean_parts(props["adm_nm"], props.get("sidonm"))
        kept = rings(feature["geometry"], KOREA_TOLERANCE)
        if not kept:
            continue
        fields = [writer.name(parts[field]) if parts[field] else None for field in FIELDS]
        korea.append((korea_country, fields, kept))

    world = []
    for feature in sources["admin1"]["features"]:
        props = feature["properties"]
        kept = rings(feature["geometry"], WORLD_TOLERANCE)
        name = (props.get("name") or "").strip()
        if not kept:
            continue
        admin = writer.name(name, translations(props)) if name else None
        world.append((country_of(props.get("iso_a2"), props.get("adm0_a3")),
                      [admin, None, None, None, None], kept))

    for feature in sources["places"]["features"]:
        props = feature["properties"]
        name = (props.get("NAME") or "").strip()
        if not name:
            continue
        lon, lat = feature["geometry"]["coordinates"][:2]
        writer.places.append((country_of(props.get("ISO_A2"), props.get("ADM0_A3")),
                              writer.name(name, translations(props, upper=True)),
                              round(lat * SCALE), round(lon * SCALE)))

    writer.layers = [korea, world]
    stats.update(korea_regions=len(korea), world_regions=len(world), places=len(writer.places),
                 strings=len(writer.strings), names=len(writer.names))
    return writer.pack(), stats


# Reading the file back: the build checks what it made, and the tests read what they packed.

class Reader:
    def __init__(self, data):
        self.data, self.at = data, 0

    def varint(self):
        value, shift = 0, 0
        while True:
            byte = self.data[self.at]
            self.at += 1
            value |= (byte & 0x7F) << shift
            if not byte & 0x80:
                return value
            shift += 7

    def zigzag(self):
        value = self.varint()
        return (value >> 1) if not value & 1 else -((value + 1) >> 1)

    def text(self):
        length = self.varint()
        text = self.data[self.at:self.at + length].decode("utf-8")
        self.at += length
        return text


def decode(data):
    """The file's contents as plain Python values."""
    if not data.startswith(MAGIC):
        raise ValueError("not a DSA boundary file")
    r = Reader(data)
    r.at = len(MAGIC)
    made_from = r.text()
    languages = [r.text() for _ in range(r.varint())]
    strings = [r.text() for _ in range(r.varint())]
    names = []
    for _ in range(r.varint()):
        names.append({languages[r.varint()]: strings[r.varint()] for _ in range(r.varint())})
    countries = []
    for _ in range(r.varint()):
        code = data[r.at:r.at + 2].decode("ascii")
        r.at += 2
        countries.append((code, names[r.varint()]))
    layers = []
    for _ in range(r.varint()):
        regions = []
        for _ in range(r.varint()):
            country = r.varint()
            fields = [r.varint() for _ in FIELDS]
            ring_list = []
            for _ in range(r.varint()):
                ring, lat, lon = [], 0, 0
                for _ in range(r.varint()):
                    lat += r.zigzag()
                    lon += r.zigzag()
                    ring.append((lat, lon))
                ring_list.append(ring)
            regions.append({
                "country": countries[country - 1][0] if country else None,
                **{field: names[value - 1] if value else None for field, value in zip(FIELDS, fields)},
                "rings": ring_list,
            })
        layers.append(regions)
    places = []
    for _ in range(r.varint()):
        country = r.varint()
        name = names[r.varint()]
        places.append({"country": countries[country - 1][0] if country else None, "name": name,
                       "lat": r.zigzag(), "lon": r.zigzag()})
    if r.at != len(data):
        raise ValueError(f"{len(data) - r.at} bytes follow the places")
    return {"made_from": made_from, "countries": countries, "layers": layers, "places": places}


def inside(rings, lat, lon):
    crossings = 0
    for ring in rings:
        j = len(ring) - 1
        for i in range(len(ring)):
            (yi, xi), (yj, xj) = ring[i], ring[j]
            if (yi > lat) != (yj > lat) and lon < (xj - xi) * (lat - yi) / (yj - yi) + xi:
                crossings += 1
            j = i
    return crossings % 2 == 1


def lookup(decoded, latitude, longitude):
    """The first region, most detailed layer first, that holds the position."""
    lat, lon = round(latitude * SCALE), round(longitude * SCALE)
    for regions in decoded["layers"]:
        for region in regions:
            if "box" not in region:
                points = [point for ring in region["rings"] for point in ring]
                region["box"] = (min(p[0] for p in points), max(p[0] for p in points),
                                 min(p[1] for p in points), max(p[1] for p in points))
            south, north, west, east = region["box"]
            if not (south <= lat <= north and west <= lon <= east):
                continue
            if inside(region["rings"], lat, lon):
                return region
    return None


def check(decoded):
    """What is wrong with the file: a known position it places wrongly."""
    problems = []
    for (lat, lon), expected in CHECKS:
        region = lookup(decoded, lat, lon)
        if region is None:
            problems.append(f"({lat}, {lon}) is in no region")
            continue
        for field, value in expected.items():
            got = region[field] if field == "country" else (region[field] or {}).get("")
            if got != value:
                problems.append(f"({lat}, {lon}): {field} is {got!r}, not {value!r}")
    return problems


def notice():
    """The attribution the sources ask for, for NOTICE.txt beside the file."""
    lines = [f"{OUTPUT_NAME}: the administrative boundaries Desktop Subsystem for Android turns",
             "positions into addresses with. It is made from:", ""]
    for source in SOURCES.values():
        lines += [f"* {source['what']}", f"  {source['url']}"]
        if source.get("notice"):
            lines += [f"  {line}" for line in source["notice"].split("\n")]
        lines.append("")
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--cache", type=Path,
                        default=Path(__file__).resolve().parents[4] / ".scratch" / "geodata")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    sources = {name: fetch(name, args.cache) for name in SOURCES}
    data, stats = build(sources)
    problems = check(decode(data))
    if problems:
        raise SystemExit("the boundary data places known positions wrongly:\n  " + "\n  ".join(problems))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(data)
    (args.out.parent / NOTICE_NAME).write_text(notice(), encoding="utf-8")
    print(f"{args.out}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")
    print(", ".join(f"{key} {value}" for key, value in stats.items()))


if __name__ == "__main__":
    sys.exit(main())
