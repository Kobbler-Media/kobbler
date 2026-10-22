#!/usr/bin/env python3
"""Rebuild the sport-want gazetteer from GeoNames.

Writes api/src/sport_want/data/{countries,cities}.tsv. Run it now and then to
pick up renamed places; the output is committed, so builds never fetch it.

    python3 scripts/build_gazetteer.py
"""

import csv
import io
import pathlib
import urllib.request
import zipfile

DUMP = "https://download.geonames.org/export/dump/"
OUT = pathlib.Path(__file__).resolve().parent.parent / "api/src/sport_want/data"
ATTRIBUTION = "# GeoNames (https://www.geonames.org), CC BY 4.0. Built by scripts/build_gazetteer.py.\n"


def fetch(name: str) -> bytes:
    with urllib.request.urlopen(DUMP + name, timeout=120) as resp:
        return resp.read()


def rows(text: str):
    for line in text.splitlines():
        if line and not line.startswith("#"):
            yield line.split("\t")


def main() -> None:
    countries = []
    for r in rows(fetch("countryInfo.txt").decode("utf-8")):
        countries.append((r[0], r[1], r[4]))

    admin1 = {r[0]: r[2] for r in rows(fetch("admin1CodesASCII.txt").decode("utf-8"))}

    with zipfile.ZipFile(io.BytesIO(fetch("cities15000.zip"))) as z:
        cities_txt = z.read("cities15000.txt").decode("utf-8")

    best: dict[tuple[str, str], tuple[int, str, str]] = {}
    for r in rows(cities_txt):
        name, cc, a1, population = r[2], r[8], r[10], int(r[14] or 0)
        region = admin1.get(f"{cc}.{a1}")
        if not region or region == name:
            continue
        key = (name.lower(), cc)
        if key not in best or population > best[key][0]:
            best[key] = (population, name, region)

    OUT.mkdir(parents=True, exist_ok=True)
    with open(OUT / "countries.tsv", "w", encoding="utf-8", newline="") as f:
        f.write(ATTRIBUTION)
        w = csv.writer(f, delimiter="\t", lineterminator="\n")
        for iso2, iso3, name in sorted(countries):
            w.writerow([iso2, iso3, name])
    with open(OUT / "cities.tsv", "w", encoding="utf-8", newline="") as f:
        f.write(ATTRIBUTION)
        w = csv.writer(f, delimiter="\t", lineterminator="\n")
        for (_, cc), (_, name, region) in sorted(best.items()):
            w.writerow([name, cc, region])


if __name__ == "__main__":
    main()
