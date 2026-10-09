#!/usr/bin/env python3
"""
Seed demo books with realistic data for UI testing.
Generates books with all metadata fields populated including:
- Genres, ratings, page counts, descriptions
- Visually distinct cover images with title text
- Realistic publisher and publication dates

Also seeds:
- Monitored authors and series
- Ingest queue items
- Proxy logs
- Discovered/wanted books
"""
import argparse
import io
import json
import random
import subprocess
import sys
import uuid
import zipfile
import zlib
import hashlib
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timedelta
from urllib import request
from urllib.error import HTTPError

try:
    from PIL import Image, ImageDraw, ImageFont
    HAS_PILLOW = True
except ImportError:
    HAS_PILLOW = False
    print("Warning: Pillow not installed. Cover generation will be slower.")
    print("Install with: pip install Pillow")

# Genre definitions with associated color palettes (RGB)
GENRES = {
    "Fantasy": {"colors": [(60, 20, 80), (100, 40, 120), (140, 60, 160)], "weight": 20},
    "Science Fiction": {"colors": [(20, 40, 80), (40, 80, 140), (60, 120, 180)], "weight": 18},
    "Mystery": {"colors": [(40, 40, 40), (80, 60, 60), (100, 80, 80)], "weight": 15},
    "Romance": {"colors": [(140, 40, 80), (180, 80, 120), (200, 120, 140)], "weight": 12},
    "Thriller": {"colors": [(60, 20, 20), (100, 40, 40), (140, 60, 60)], "weight": 14},
    "Historical Fiction": {"colors": [(100, 80, 40), (140, 110, 60), (170, 140, 90)], "weight": 10},
    "Literary Fiction": {"colors": [(60, 80, 60), (90, 110, 90), (120, 140, 120)], "weight": 8},
    "Horror": {"colors": [(30, 10, 30), (50, 20, 50), (80, 30, 60)], "weight": 8},
    "Young Adult": {"colors": [(100, 60, 140), (140, 100, 180), (180, 140, 200)], "weight": 10},
    "Adventure": {"colors": [(80, 100, 40), (120, 140, 60), (160, 180, 80)], "weight": 10},
}

SUBGENRES = {
    "Fantasy": ["Epic Fantasy", "Urban Fantasy", "Dark Fantasy", "High Fantasy", "Sword & Sorcery"],
    "Science Fiction": ["Space Opera", "Cyberpunk", "Hard Sci-Fi", "Dystopian", "First Contact"],
    "Mystery": ["Cozy Mystery", "Police Procedural", "Noir", "Amateur Sleuth", "Locked Room"],
    "Romance": ["Contemporary Romance", "Historical Romance", "Paranormal Romance", "Romantic Suspense"],
    "Thriller": ["Psychological Thriller", "Legal Thriller", "Medical Thriller", "Spy Thriller"],
    "Historical Fiction": ["Medieval", "Victorian", "World War II", "Ancient Rome", "Renaissance"],
    "Literary Fiction": ["Contemporary", "Experimental", "Family Saga", "Magical Realism"],
    "Horror": ["Gothic", "Supernatural", "Psychological Horror", "Cosmic Horror", "Slasher"],
    "Young Adult": ["Coming of Age", "Dystopian YA", "Fantasy YA", "Contemporary YA"],
    "Adventure": ["Survival", "Treasure Hunt", "Exploration", "Quest"],
}

# Expanded word lists for title generation
TITLE_PATTERNS = [
    "The {adj} {noun}",
    "{noun} of {noun2}",
    "The {noun}'s {noun2}",
    "{adj} {noun}",
    "The Last {noun}",
    "A {noun} of {noun2} and {noun3}",
    "{noun} Rising",
    "The {adj} {noun} of {place}",
    "Beyond the {noun}",
    "{noun} in the {noun2}",
    "The {noun} King",
    "The {noun} Queen",
    "House of {noun}",
    "City of {noun}",
    "Children of {noun}",
]

ADJECTIVES = [
    "Ancient", "Broken", "Crimson", "Dark", "Eternal", "Fallen", "Golden", "Hidden",
    "Iron", "Jade", "Kindred", "Lost", "Mystic", "Noble", "Obsidian", "Phantom",
    "Quiet", "Rising", "Shadow", "Twilight", "Unseen", "Violet", "Wandering",
    "Silent", "Burning", "Frozen", "Shattered", "Hollow", "Sacred", "Cursed",
    "Forgotten", "Endless", "Silver", "Emerald", "Sapphire", "Ruby", "Onyx",
    "Starless", "Moonlit", "Sunken", "Fading", "Radiant", "Veiled", "Wicked",
]

NOUNS = [
    "Alliance", "Blade", "Crown", "Dragon", "Empire", "Flame", "Gate", "Heart",
    "Island", "Journey", "Kingdom", "Legacy", "Moon", "Night", "Oracle", "Prince",
    "Queen", "Raven", "Storm", "Throne", "Uprising", "Void", "War", "Exile",
    "Abyss", "Beacon", "Citadel", "Dawn", "Echo", "Fortress", "Guild", "Haven",
    "Inheritance", "Judgment", "Knight", "Labyrinth", "Memory", "Nexus", "Oath",
    "Pact", "Quest", "Rebellion", "Scepter", "Tower", "Unity", "Veil", "Whisper",
    "Star", "Blood", "Bone", "Shadow", "Light", "Truth", "Lies", "Secrets",
    "Time", "Death", "Life", "Magic", "Power", "Sword", "Shield", "Song",
]

PLACES = [
    "Eldoria", "Ravenmoor", "Shadowfall", "Crystalheim", "Ironhold", "Thornwood",
    "Stormhaven", "Nighthollow", "Sunspire", "Frostmere", "Goldcrest", "Silverpeak",
    "Darkwater", "Starfall", "Moonridge", "Dragonstone", "Winterfell", "Summervale",
]

SERIES_NAMES = [
    "Chronicles of {place}",
    "The {place} Saga",
    "Tales of the {noun}",
    "The {adj} {noun} Series",
    "Legends of {place}",
    "The {noun} Wars",
    "Whispers of {noun}",
    "The {noun}'s Path",
    "Secrets of the {noun}",
    "The {noun} Rising",
    "Bonds of {noun}",
    "The {adj} Empire",
    "Echoes of {noun}",
    "The Last {noun}",
    "Guardians of {noun}",
    "The {adj} Order",
    "Realms of {noun}",
    "The {noun} Circle",
]

PUBLISHERS = [
    "Tor Books", "Del Rey", "Ace Books", "DAW Books", "Orbit", "Gollancz",
    "Baen Books", "Harper Voyager", "Saga Press", "Berkley", "Bantam Spectra",
    "Angry Robot", "Tachyon Publications", "Subterranean Press", "Titan Books",
    "Solaris", "Jo Fletcher Books", "Head of Zeus", "Hodder & Stoughton",
    "Pan Macmillan", "Penguin Random House", "Simon & Schuster", "Hachette",
]

AUTHOR_FIRST = [
    "James", "Sarah", "Michael", "Emma", "David", "Olivia", "Robert", "Sophia",
    "William", "Isabella", "Thomas", "Mia", "Charles", "Charlotte", "Joseph", "Amelia",
    "Daniel", "Emily", "Matthew", "Elizabeth", "Andrew", "Victoria", "Christopher", "Grace",
    "Brandon", "Samantha", "Patrick", "Katherine", "Robin", "Morgan", "Jordan", "Alex",
    "N.K.", "V.E.", "R.F.", "J.R.R.", "G.R.R.", "C.S.", "J.K.", "T.H.",
]

AUTHOR_LAST = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
    "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson", "Thomas",
    "Blackwood", "Thornton", "Sterling", "Ashford", "Whitmore", "Chambers", "Hartwell", "Kingsley",
    "Sanderson", "Rothfuss", "Martin", "Tolkien", "Lewis", "Jemisin", "Schwab", "Bardugo",
]

DESCRIPTION_TEMPLATES = [
    "In a world where {concept}, {protagonist} must {quest} before {threat}. A {adj} tale of {theme1} and {theme2}.",
    "{protagonist} never expected to {discovery}. But when {event}, they must journey to {place} to {goal}. An epic story of {theme1}, {theme2}, and the {noun} that binds us all.",
    "When {event}, {protagonist} discovers they are the key to {concept}. With {companion} by their side, they must face {antagonist} and uncover the truth about {mystery}.",
    "The {noun} has been {adj2} for centuries. Now, {protagonist} holds the key to its {noun2}. But {antagonist} will stop at nothing to claim it first.",
    "{protagonist} has always known they were different. But nothing could prepare them for the {discovery} that would change everything. A {adj} novel about {theme1} and finding one's place in the {noun}.",
    "In the {adj} city of {place}, {protagonist} ekes out a living as a {profession}. But when {event}, they're drawn into a dangerous game of {theme1} and {theme2}.",
    "They said the {noun} was just a legend. {protagonist} is about to prove them wrong. A breathtaking adventure through {place} where nothing is as it seems.",
    "War has ravaged {place} for generations. {protagonist}, a {profession}, wants nothing more than peace. But when {event}, they must choose between {choice1} and {choice2}.",
]

CONCEPTS = [
    "magic flows through bloodlines", "the gods walk among mortals", "technology and sorcery intertwine",
    "the boundary between worlds grows thin", "ancient powers awaken", "prophecies shape destiny",
    "the dead don't stay buried", "memories can be stolen", "names hold power",
]

PROTAGONISTS = [
    "a young apprentice", "a disgraced knight", "a cunning thief", "an unlikely hero",
    "a reluctant warrior", "a scholar with a secret", "a princess in hiding", "a merchant's daughter",
    "an orphan with mysterious powers", "a soldier seeking redemption", "a healer with a dark past",
]

QUESTS = [
    "uncover an ancient secret", "prevent a devastating war", "find a legendary artifact",
    "break an unbreakable curse", "unite the fractured kingdoms", "stop a rising darkness",
    "save the one they love", "discover their true heritage", "master forbidden magic",
]

THREATS = [
    "darkness consumes the land", "an empire rises to conquer all", "the old gods return",
    "time runs out", "everything they know is destroyed", "their enemies close in",
]

THEMES = [
    "love", "betrayal", "redemption", "sacrifice", "power", "family", "destiny", "courage",
    "friendship", "loss", "hope", "revenge", "identity", "truth", "loyalty", "freedom",
]

EVENTS = [
    "a mysterious stranger arrives", "an ancient artifact is discovered", "a king is murdered",
    "war breaks out", "a plague sweeps the land", "the barriers between worlds shatter",
    "a prophecy begins to unfold", "old enemies resurface", "magic returns to the world",
]

PROFESSIONS = [
    "blacksmith", "scribe", "healer", "soldier", "merchant", "spy", "assassin", "scholar",
    "sailor", "farmer", "noble", "priest", "mage", "thief", "hunter", "bard",
]

# --- Movie generation data ---

MOVIE_TITLE_PATTERNS = [
    "The {adj} {noun}",
    "{noun} of {noun2}",
    "The Last {noun}",
    "{adj} {noun}: {noun2}",
    "Operation {noun}",
    "The {noun} Project",
    "{noun} Rising",
    "Beyond {noun}",
    "The {noun} Protocol",
    "{adj} {noun} Falls",
    "Under the {adj} {noun}",
    "{noun} at Midnight",
    "The {noun} Conspiracy",
    "Into the {noun}",
    "{noun} Zero",
]

MOVIE_GENRES = {
    "Action": {"colors": [(180, 40, 40), (200, 60, 30), (160, 30, 30)], "weight": 20},
    "Comedy": {"colors": [(200, 180, 40), (220, 200, 60), (180, 160, 30)], "weight": 15},
    "Drama": {"colors": [(60, 80, 120), (80, 100, 140), (50, 70, 110)], "weight": 18},
    "Sci-Fi": {"colors": [(20, 60, 120), (40, 80, 160), (10, 50, 100)], "weight": 15},
    "Horror": {"colors": [(40, 10, 40), (60, 20, 50), (30, 5, 30)], "weight": 10},
    "Thriller": {"colors": [(60, 60, 60), (80, 70, 70), (50, 50, 50)], "weight": 14},
    "Animation": {"colors": [(100, 180, 220), (120, 200, 240), (80, 160, 200)], "weight": 8},
    "Documentary": {"colors": [(80, 120, 80), (100, 140, 100), (60, 100, 60)], "weight": 5},
}

DIRECTORS = [
    "Christopher Nolan", "Denis Villeneuve", "Greta Gerwig", "Jordan Peele",
    "Bong Joon-ho", "Chloe Zhao", "Martin Scorsese", "Wes Anderson",
    "Kathryn Bigelow", "Taika Waititi", "Barry Jenkins", "Ava DuVernay",
    "David Fincher", "Guillermo del Toro", "Sofia Coppola", "Ridley Scott",
    "Quentin Tarantino", "The Coen Brothers", "Alfonso Cuaron", "Spike Lee",
]

STUDIOS = [
    "Universal Pictures", "Warner Bros.", "A24", "Paramount Pictures",
    "20th Century Studios", "Sony Pictures", "Lionsgate", "Focus Features",
    "Searchlight Pictures", "NEON", "STX Entertainment", "Annapurna Pictures",
    "Blumhouse Productions", "Legendary Entertainment", "New Line Cinema",
]

CONTENT_RATINGS = ["G", "PG", "PG-13", "PG-13", "PG-13", "R", "R", "NR"]

MOVIE_DESCRIPTION_TEMPLATES = [
    "In a world where {concept}, one person must {quest} before {threat}. A gripping {genre} that will keep you on the edge of your seat.",
    "When {event}, {protagonist} is forced to confront {mystery}. Directed by {director}, this {genre} explores themes of {theme1} and {theme2}.",
    "{protagonist} has always lived a quiet life. But when {event}, everything changes. A {adj} {genre} about {theme1} and the price of {theme2}.",
    "Based on true events. {protagonist} faces impossible odds when {event}. A powerful {genre} that examines {theme1} in a world consumed by {theme2}.",
    "From visionary director {director} comes a {adj} tale of {theme1}. When {event}, {protagonist} must choose between {choice1} and {choice2}.",
]

# --- TV show generation data ---

TV_TITLE_PATTERNS = [
    "The {adj} {noun}",
    "{noun} Files",
    "Breaking {noun}",
    "{place}: {noun}",
    "The {noun} Report",
    "{adj} {noun}",
    "House of {noun}",
    "{noun} & {noun2}",
    "The {noun} Diaries",
    "{noun} Heights",
    "Agent {noun}",
    "The {adj} Hour",
    "{noun} Academy",
    "Dark {noun}",
    "True {noun}",
]

TV_NETWORKS = [
    "HBO", "Netflix", "Apple TV+", "Amazon Prime", "FX", "AMC",
    "Hulu", "Disney+", "Peacock", "Paramount+", "BBC", "Showtime",
    "CBS", "NBC", "ABC", "The CW", "Starz", "Crunchyroll",
]

TV_STATUSES = ["Returning Series", "Ended", "Canceled", "Returning Series", "Ended", "Returning Series"]

TV_CONTENT_RATINGS = ["TV-Y", "TV-Y7", "TV-G", "TV-PG", "TV-14", "TV-14", "TV-MA", "TV-MA"]


ACTOR_FIRST = [
    "Chris", "Emma", "Ryan", "Scarlett", "Tom", "Natalie", "Robert", "Margot",
    "Leonardo", "Cate", "Brad", "Meryl", "Denzel", "Viola", "Morgan", "Judi",
    "Idris", "Lupita", "Oscar", "Saoirse", "Samuel", "Florence", "Timothee", "Zendaya",
    "Jake", "Amy", "Benedict", "Tilda", "Joaquin", "Olivia", "Keanu", "Michelle",
]

ACTOR_LAST = [
    "Evans", "Stone", "Gosling", "Johansson", "Hanks", "Portman", "Downey", "Robbie",
    "DiCaprio", "Blanchett", "Pitt", "Streep", "Washington", "Davis", "Freeman", "Dench",
    "Elba", "Nyong'o", "Isaac", "Ronan", "Jackson", "Pugh", "Chalamet", "Coleman",
    "Gyllenhaal", "Adams", "Cumberbatch", "Swinton", "Phoenix", "Colman", "Reeves", "Yeoh",
]

CHARACTER_FIRST = [
    "Jack", "Sarah", "James", "Elena", "Max", "Lena", "Alex", "Maya",
    "Marcus", "Zara", "Viktor", "Isla", "Kai", "Nova", "Dante", "Aria",
    "Reese", "Morgan", "Quinn", "Eden", "Blake", "Harper", "Logan", "Sage",
]

CHARACTER_LAST = [
    "Hartwell", "Voss", "Chen", "Okafor", "Reyes", "Novak", "Kim", "Sterling",
    "Frost", "Rivera", "Blake", "Torres", "Kane", "Moreau", "Ashford", "Volkov",
]

DEPARTMENTS = ["Production", "Art", "Sound", "Camera", "Editing", "Visual Effects", "Costume Design"]
JOBS_BY_DEPT = {
    "Directing": ["Director", "First Assistant Director"],
    "Writing": ["Writer", "Screenplay", "Story"],
    "Production": ["Producer", "Executive Producer", "Co-Producer"],
    "Art": ["Production Designer", "Art Director"],
    "Sound": ["Composer", "Sound Designer", "Music Supervisor"],
    "Camera": ["Director of Photography", "Camera Operator"],
    "Editing": ["Editor", "Colorist"],
    "Visual Effects": ["VFX Supervisor", "VFX Producer"],
    "Costume Design": ["Costume Designer"],
}


def generate_movie(index, rng):
    """Generate a complete movie with all metadata."""
    # Pick primary genre (weighted)
    genre_weights = [(g, info["weight"]) for g, info in MOVIE_GENRES.items()]
    total_weight = sum(w for _, w in genre_weights)
    r = rng.random() * total_weight
    cumulative = 0
    primary_genre = "Drama"
    for genre, weight in genre_weights:
        cumulative += weight
        if r <= cumulative:
            primary_genre = genre
            break

    genres = [primary_genre]
    if rng.random() < 0.5:
        other = rng.choice(list(MOVIE_GENRES.keys()))
        if other != primary_genre:
            genres.append(other)

    pattern = rng.choice(MOVIE_TITLE_PATTERNS)
    title = pattern.format(
        adj=rng.choice(ADJECTIVES),
        noun=rng.choice(NOUNS),
        noun2=rng.choice(NOUNS),
        place=rng.choice(PLACES),
    )

    director = rng.choice(DIRECTORS)
    years_ago = int(rng.expovariate(0.2))
    years_ago = min(years_ago, 40)
    year = 2025 - years_ago
    runtime = int(max(80, min(210, rng.gauss(120, 25))))
    rating = round(max(1.0, min(10.0, rng.gauss(6.8, 1.5))), 1)
    rating_count = max(50, min(500000, int(rng.expovariate(0.00005))))
    content_rating = rng.choice(CONTENT_RATINGS)

    template = rng.choice(MOVIE_DESCRIPTION_TEMPLATES)
    themes = list(THEMES)
    rng.shuffle(themes)
    description = template.format(
        concept=rng.choice(CONCEPTS),
        protagonist=rng.choice(PROTAGONISTS),
        quest=rng.choice(QUESTS),
        threat=rng.choice(THREATS),
        genre=primary_genre.lower(),
        event=rng.choice(EVENTS),
        mystery=rng.choice(["their past", "a hidden truth", "a global conspiracy", "a deadly secret"]),
        director=director,
        adj=rng.choice(ADJECTIVES).lower(),
        theme1=themes[0],
        theme2=themes[1],
        choice1=rng.choice(["love", "duty", "honor", "family"]),
        choice2=rng.choice(["survival", "vengeance", "the greater good", "freedom"]),
    )

    tmdb_id = str(10000 + index * 7 + rng.randint(0, 5))
    imdb_id = f"tt{7000000 + index * 3 + rng.randint(0, 2)}"

    return {
        "title": title,
        "year": year,
        "runtime": runtime,
        "genres": genres,
        "description": description,
        "rating": rating,
        "rating_count": rating_count,
        "content_rating": content_rating,
        "director": director,
        "identifiers": {"tmdb_id": tmdb_id, "imdb_id": imdb_id},
        "primary_genre": primary_genre,
    }


def generate_tv_show(index, rng, count):
    """Generate a complete TV show with seasons and episodes."""
    # Stress-test shows for large counts
    if count >= 20 and index == 0:
        return {
            "title": "The Endless Saga",
            "year": 1975,
            "status": "Ended",
            "network": "BBC",
            "genres": ["Drama", "Sci-Fi"],
            "description": "A legendary series spanning five decades. The longest-running drama in television history.",
            "rating": 7.2,
            "rating_count": 85000,
            "content_rating": "TV-PG",
            "identifiers": {"tmdb_id": "99001", "imdb_id": "tt9900001"},
            "seasons": [{"number": s, "episode_count": 10} for s in range(1, 51)],
        }

    if count >= 20 and index == 1:
        return {
            "title": "Daily News Hour",
            "year": 2020,
            "status": "Returning Series",
            "network": "Netflix",
            "genres": ["Documentary", "Drama"],
            "description": "A daily deep-dive into global events, with over 200 episodes per season.",
            "rating": 6.5,
            "rating_count": 12000,
            "content_rating": "TV-14",
            "identifiers": {"tmdb_id": "99002", "imdb_id": "tt9900002"},
            "seasons": [
                {"number": 1, "episode_count": 220},
                {"number": 2, "episode_count": 210},
            ],
        }

    # Normal show generation
    genre_weights = [(g, info["weight"]) for g, info in MOVIE_GENRES.items()]
    total_weight = sum(w for _, w in genre_weights)
    r = rng.random() * total_weight
    cumulative = 0
    primary_genre = "Drama"
    for genre, weight in genre_weights:
        cumulative += weight
        if r <= cumulative:
            primary_genre = genre
            break

    genres = [primary_genre]
    if rng.random() < 0.5:
        other = rng.choice(list(MOVIE_GENRES.keys()))
        if other != primary_genre:
            genres.append(other)

    pattern = rng.choice(TV_TITLE_PATTERNS)
    title = pattern.format(
        adj=rng.choice(ADJECTIVES),
        noun=rng.choice(NOUNS),
        noun2=rng.choice(NOUNS),
        place=rng.choice(PLACES),
    )

    years_ago = int(rng.expovariate(0.25))
    years_ago = min(years_ago, 30)
    year = 2025 - years_ago
    status = rng.choice(TV_STATUSES)
    network = rng.choice(TV_NETWORKS)
    rating = round(max(1.0, min(10.0, rng.gauss(7.2, 1.2))), 1)
    rating_count = max(100, min(300000, int(rng.expovariate(0.0001))))
    content_rating = rng.choice(TV_CONTENT_RATINGS)

    template = rng.choice(MOVIE_DESCRIPTION_TEMPLATES)
    themes = list(THEMES)
    rng.shuffle(themes)
    description = template.format(
        concept=rng.choice(CONCEPTS),
        protagonist=rng.choice(PROTAGONISTS),
        quest=rng.choice(QUESTS),
        threat=rng.choice(THREATS),
        genre=primary_genre.lower(),
        event=rng.choice(EVENTS),
        mystery=rng.choice(["their past", "a hidden truth", "a conspiracy", "a deadly secret"]),
        director=rng.choice(DIRECTORS),
        adj=rng.choice(ADJECTIVES).lower(),
        theme1=themes[0],
        theme2=themes[1],
        choice1=rng.choice(["love", "duty", "honor", "family"]),
        choice2=rng.choice(["survival", "vengeance", "the greater good", "freedom"]),
    )

    tmdb_id = str(50000 + index * 7 + rng.randint(0, 5))
    imdb_id = f"tt{8000000 + index * 3 + rng.randint(0, 2)}"

    # Generate seasons: 1-8 seasons, 6-24 episodes each
    num_seasons = rng.randint(1, 8)
    seasons = []
    for s in range(1, num_seasons + 1):
        ep_count = rng.randint(6, 24)
        seasons.append({"number": s, "episode_count": ep_count})

    return {
        "title": title,
        "year": year,
        "status": status,
        "network": network,
        "genres": genres,
        "description": description,
        "rating": rating,
        "rating_count": rating_count,
        "content_rating": content_rating,
        "identifiers": {"tmdb_id": tmdb_id, "imdb_id": imdb_id},
        "seasons": seasons,
    }


def seed_movies(db_container, rng, count):
    """Seed movies via batched SQL inserts. Returns list of {id, title, want_status}."""
    results = []
    statements = []
    BATCH_SIZE = 200

    for i in range(count):
        movie = generate_movie(i, rng)
        movie_id = str(uuid.uuid4())

        # Status distribution: ~35% library, ~35% wanted, ~30% tracked
        roll = rng.random()
        if roll < 0.35:
            is_in_library = True
            want_status = "none"
        elif roll < 0.70:
            is_in_library = False
            want_status = "wanted"
        else:
            is_in_library = False
            want_status = "none"

        genres_sql = "ARRAY[" + ",".join(f"'{g}'" for g in movie["genres"]) + "]::text[]"
        identifiers_json = json.dumps(movie["identifiers"]).replace("'", "''")
        title_sql = movie["title"].replace("'", "''")
        desc_sql = movie["description"].replace("'", "''")
        sort_title = movie["title"].lower().removeprefix("the ").strip()
        sort_title_sql = sort_title.replace("'", "''")
        release_date = f"{movie['year']}-{rng.randint(1,12):02d}-{rng.randint(1,28):02d}"

        statements.append(f"""INSERT INTO movies (id, title, sort_title, year, runtime, description, genres,
            rating, rating_count, content_rating, identifiers, is_in_library, want_status,
            release_date, is_released, created_at, updated_at)
        VALUES (
            '{movie_id}', '{title_sql}', '{sort_title_sql}', {movie['year']}, {movie['runtime']},
            '{desc_sql}', {genres_sql},
            {movie['rating']}, {movie['rating_count']}, '{movie['content_rating']}',
            '{identifiers_json}'::jsonb, {str(is_in_library).upper()}, '{want_status}',
            '{release_date}', TRUE, NOW(), NOW()
        ) ON CONFLICT DO NOTHING;""")

        if is_in_library:
            file_id = str(uuid.uuid4())
            statements.append(f"""INSERT INTO movie_files (id, movie_id, file_path, file_size, format, quality, available, created_at)
            VALUES (
                '{file_id}', '{movie_id}',
                '/media/movies/{title_sql} ({movie["year"]})/{title_sql}.mkv',
                {rng.randint(700_000_000, 8_000_000_000)},
                'mkv',
                '{{"resolution": "{rng.choice(["720p", "1080p", "1080p", "2160p"])}", "codec": "{rng.choice(["h264", "h265"])}"}}'::jsonb,
                TRUE, NOW()
            ) ON CONFLICT DO NOTHING;""")

        results.append({"id": movie_id, "title": movie["title"], "want_status": want_status})

        if len(statements) >= BATCH_SIZE:
            execute_sql_batch(db_container, statements)
            statements = []

    if statements:
        execute_sql_batch(db_container, statements)

    return results


def seed_tv_shows(db_container, rng, count):
    """Seed TV shows with seasons and episodes via batched SQL. Returns summary dict."""
    total_episodes = 0
    wanted_episodes = []
    shows = []
    statements = []
    BATCH_SIZE = 500

    def flush():
        nonlocal statements
        if statements:
            execute_sql_batch(db_container, statements)
            statements = []

    for i in range(count):
        show = generate_tv_show(i, rng, count)
        show_id = str(uuid.uuid4())

        # Show monitoring distribution: ~40% all, ~20% new, ~40% not monitored
        roll = rng.random()
        if roll < 0.40:
            is_monitored = True
            auto_want_mode = "all"
        elif roll < 0.60:
            is_monitored = True
            auto_want_mode = "new"
        else:
            is_monitored = False
            auto_want_mode = "none"

        genres_sql = "ARRAY[" + ",".join(f"'{g}'" for g in show["genres"]) + "]::text[]"
        identifiers_json = json.dumps(show["identifiers"]).replace("'", "''")
        title_sql = show["title"].replace("'", "''")
        desc_sql = show["description"].replace("'", "''")
        sort_title = show["title"].lower().removeprefix("the ").strip()
        sort_title_sql = sort_title.replace("'", "''")
        network_sql = show.get("network", "").replace("'", "''") if show.get("network") else "NULL"
        network_val = f"'{network_sql}'" if network_sql != "NULL" else "NULL"
        status_map = {
            "returning_series": "continuing",
            "ended": "ended",
            "canceled": "canceled",
        }
        raw_status = show.get("status", "Returning Series").lower().replace(" ", "_")
        db_status = status_map.get(raw_status, "continuing")

        total_ep_count = sum(s["episode_count"] for s in show["seasons"])
        first_aired = f"'{show['year']}-01-15'"

        statements.append(f"""INSERT INTO tv_shows (id, title, sort_title, year, status, description, genres,
            rating, rating_count, content_rating, network, identifiers,
            is_monitored, auto_want_mode, total_episode_count, first_aired,
            created_at, updated_at)
        VALUES (
            '{show_id}', '{title_sql}', '{sort_title_sql}', {show['year']},
            '{db_status}', '{desc_sql}', {genres_sql},
            {show['rating']}, {show['rating_count']}, '{show.get("content_rating", "TV-14")}',
            {network_val}, '{identifiers_json}'::jsonb,
            {str(is_monitored).upper()}, '{auto_want_mode}', {total_ep_count},
            {first_aired},
            NOW(), NOW()
        ) ON CONFLICT DO NOTHING;""")

        show_seasons = []

        num_seasons = len(show["seasons"])
        for season_info in show["seasons"]:
            season_num = season_info["number"]
            season_id = str(uuid.uuid4())
            show_seasons.append({"id": season_id, "number": season_num})
            is_latest = (season_num == num_seasons)

            statements.append(f"""INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order,
                title, episode_count, is_monitored, is_special, created_at)
            VALUES (
                '{season_id}', '{show_id}', '{season_num}', {season_num},
                'Season {season_num}', {season_info["episode_count"]},
                {str(is_monitored).upper()}, FALSE, NOW()
            ) ON CONFLICT DO NOTHING;""")

            for ep_num in range(1, season_info["episode_count"] + 1):
                episode_id = str(uuid.uuid4())
                total_episodes += 1

                ep_title = f"Episode {ep_num}"
                ep_title_sql = ep_title.replace("'", "''")
                air_year = show["year"] + season_num - 1
                air_date = f"'{air_year}-{rng.randint(1,12):02d}-{rng.randint(1,28):02d}'"
                runtime = rng.choice([22, 30, 42, 45, 50, 60])

                if not is_monitored:
                    ep_in_library = False
                    ep_want = "none"
                elif is_latest:
                    ep_roll = rng.random()
                    if ep_roll < 0.4:
                        ep_in_library = True
                        ep_want = "none"
                    elif ep_roll < 0.75:
                        ep_in_library = False
                        ep_want = "wanted"
                    else:
                        ep_in_library = False
                        ep_want = "none"
                else:
                    ep_roll = rng.random()
                    if ep_roll < 0.85:
                        ep_in_library = True
                        ep_want = "none"
                    else:
                        ep_in_library = False
                        ep_want = "none"

                statements.append(f"""INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order,
                    title, air_date, runtime, is_in_library, want_status, created_at, updated_at)
                VALUES (
                    '{episode_id}', '{season_id}', '{ep_num}', {ep_num},
                    '{ep_title_sql}', {air_date}, {runtime},
                    {str(ep_in_library).upper()}, '{ep_want}',
                    NOW(), NOW()
                ) ON CONFLICT DO NOTHING;""")

                if ep_in_library:
                    file_id = str(uuid.uuid4())
                    statements.append(f"""INSERT INTO tv_episode_files (id, episode_id, file_path, file_size, format,
                        quality, available, created_at)
                    VALUES (
                        '{file_id}', '{episode_id}',
                        '/media/tv/{title_sql}/Season {season_num:02d}/S{season_num:02d}E{ep_num:02d}.mkv',
                        {rng.randint(200_000_000, 2_000_000_000)},
                        'mkv',
                        '{{"resolution": "{rng.choice(["720p", "1080p", "1080p"])}", "codec": "h264"}}'::jsonb,
                        TRUE, NOW()
                    ) ON CONFLICT DO NOTHING;""")

                if ep_want == "wanted":
                    wanted_episodes.append({
                        "episode_id": episode_id,
                        "show_id": show_id,
                        "show_title": show["title"],
                        "season_id": season_id,
                        "season_identifier": str(season_num),
                        "episode_identifier": str(ep_num),
                    })

            if len(statements) >= BATCH_SIZE:
                flush()

        shows.append({"id": show_id, "title": show["title"], "seasons": show_seasons})

    flush()

    return {
        "show_count": count,
        "episode_count": total_episodes,
        "wanted_episodes": wanted_episodes,
        "shows": shows,
    }


def make_cover_png(title, author, genre, width=400, height=600, seed=None):
    """Generate a visually distinct cover with gradient background and text."""
    if seed is None:
        seed = hash(title)
    rng = random.Random(seed)

    # Get colors based on genre
    genre_info = GENRES.get(genre, GENRES["Fantasy"])
    colors = genre_info["colors"]

    # Add some variation to base color
    base_color = list(rng.choice(colors))
    for i in range(3):
        base_color[i] = max(0, min(255, base_color[i] + rng.randint(-20, 20)))

    if HAS_PILLOW:
        return _make_cover_pillow(title, author, base_color, width, height, rng)
    else:
        return _make_cover_manual(base_color, width, height)


def _make_cover_pillow(title, author, base_color, width, height, rng):
    """Generate cover using Pillow (fast)."""
    # Create gradient background
    img = Image.new('RGB', (width, height))
    draw = ImageDraw.Draw(img)

    # Draw gradient
    for y in range(height):
        progress = y / max(height - 1, 1)
        shade_mult = 0.6 + 0.8 * progress
        r = int(max(0, min(255, base_color[0] * shade_mult)))
        g = int(max(0, min(255, base_color[1] * shade_mult)))
        b = int(max(0, min(255, base_color[2] * shade_mult)))
        draw.line([(0, y), (width, y)], fill=(r, g, b))

    # Draw lighter band in middle
    band_start = int(height * 0.3)
    band_end = int(height * 0.5)
    for y in range(band_start, band_end):
        # Get current color and lighten
        pixel = img.getpixel((width // 2, y))
        lighter = tuple(min(255, int(c * 1.3)) for c in pixel)
        draw.line([(0, y), (width, y)], fill=lighter)

    # Try to add text (use default font if custom fonts unavailable)
    try:
        # Try to load a nice font, fall back to default
        try:
            title_font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 28)
            author_font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 18)
        except (OSError, IOError):
            title_font = ImageFont.load_default()
            author_font = ImageFont.load_default()

        # Draw title (wrap if too long)
        title_y = int(height * 0.35)
        words = title.split()
        lines = []
        current_line = []
        for word in words:
            test_line = ' '.join(current_line + [word])
            bbox = draw.textbbox((0, 0), test_line, font=title_font)
            if bbox[2] > width - 40:
                if current_line:
                    lines.append(' '.join(current_line))
                current_line = [word]
            else:
                current_line.append(word)
        if current_line:
            lines.append(' '.join(current_line))

        for i, line in enumerate(lines[:3]):  # Max 3 lines
            bbox = draw.textbbox((0, 0), line, font=title_font)
            text_width = bbox[2] - bbox[0]
            x = (width - text_width) // 2
            y = title_y + i * 32
            # Draw shadow
            draw.text((x + 2, y + 2), line, font=title_font, fill=(0, 0, 0, 128))
            draw.text((x, y), line, font=title_font, fill=(255, 255, 255))

        # Draw author
        author_y = int(height * 0.75)
        bbox = draw.textbbox((0, 0), author, font=author_font)
        text_width = bbox[2] - bbox[0]
        x = (width - text_width) // 2
        draw.text((x + 1, author_y + 1), author, font=author_font, fill=(0, 0, 0, 128))
        draw.text((x, author_y), author, font=author_font, fill=(220, 220, 220))

    except Exception:
        pass  # Skip text if fonts unavailable

    # Convert to PNG bytes
    buffer = io.BytesIO()
    img.save(buffer, format='PNG', optimize=True)
    return buffer.getvalue()


def _crc(chunk_type, data):
    return zlib.crc32(chunk_type + data) & 0xFFFFFFFF


def _png_chunk(chunk_type, data):
    length = len(data).to_bytes(4, "big")
    crc = _crc(chunk_type, data).to_bytes(4, "big")
    return length + chunk_type + data + crc


def _make_cover_manual(base_color, width, height):
    """Generate cover manually without Pillow (slow fallback)."""
    rows = []
    for y in range(height):
        progress = y / max(height - 1, 1)
        row_bytes = []
        for x in range(width):
            x_var = (x / width - 0.5) * 0.2
            shade_mult = 0.6 + 0.8 * progress
            r = int(max(0, min(255, base_color[0] * shade_mult + x_var * 30)))
            g = int(max(0, min(255, base_color[1] * shade_mult + x_var * 30)))
            b = int(max(0, min(255, base_color[2] * shade_mult + x_var * 30)))
            row_bytes.extend([r, g, b])
        rows.append(b"\x00" + bytes(row_bytes))

    band_start = int(height * 0.3)
    band_end = int(height * 0.5)
    for y in range(band_start, band_end):
        row = bytearray(rows[y])
        for x in range(1, len(row), 3):
            row[x] = min(255, int(row[x] * 1.3))
            row[x+1] = min(255, int(row[x+1] * 1.3))
            row[x+2] = min(255, int(row[x+2] * 1.3))
        rows[y] = bytes(row)

    raw = b"".join(rows)
    compressed = zlib.compress(raw, level=6)

    signature = b"\x89PNG\r\n\x1a\n"
    ihdr = width.to_bytes(4, "big") + height.to_bytes(4, "big") + b"\x08\x02\x00\x00\x00"
    return b"".join([
        signature,
        _png_chunk(b"IHDR", ihdr),
        _png_chunk(b"IDAT", compressed),
        _png_chunk(b"IEND", b""),
    ])


def make_poster_png(title, genre, width=400, height=600, seed=None):
    """Generate a movie/TV poster PNG (reuses book cover logic with title only)."""
    return make_cover_png(title, genre, genre, width=width, height=height, seed=seed)


def make_backdrop_png(title, genre, width=800, height=450, seed=None):
    """Generate a movie/TV backdrop PNG (wider aspect ratio, title in lower-left)."""
    if seed is None:
        seed = hash(title)
    rng = random.Random(seed)

    genre_info = MOVIE_GENRES.get(genre, MOVIE_GENRES.get("Drama", {"colors": [(60, 80, 120)]}))
    colors = genre_info["colors"]

    base_color = list(rng.choice(colors))
    for i in range(3):
        base_color[i] = max(0, min(255, base_color[i] + rng.randint(-20, 20)))

    if HAS_PILLOW:
        img = Image.new('RGB', (width, height))
        draw = ImageDraw.Draw(img)

        # Horizontal gradient (left to right)
        for x in range(width):
            progress = x / max(width - 1, 1)
            shade_mult = 0.5 + 0.8 * progress
            r = int(max(0, min(255, base_color[0] * shade_mult)))
            g = int(max(0, min(255, base_color[1] * shade_mult)))
            b = int(max(0, min(255, base_color[2] * shade_mult)))
            draw.line([(x, 0), (x, height)], fill=(r, g, b))

        # Add title text in lower-left
        try:
            try:
                font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 24)
            except (OSError, IOError):
                font = ImageFont.load_default()
            x, y = 20, height - 50
            draw.text((x + 2, y + 2), title[:40], font=font, fill=(0, 0, 0, 128))
            draw.text((x, y), title[:40], font=font, fill=(255, 255, 255))
        except Exception:
            pass

        buffer = io.BytesIO()
        img.save(buffer, format='PNG', optimize=True)
        return buffer.getvalue()
    else:
        return _make_cover_manual(base_color, width, height)


def make_epub_bytes(title, author, description=""):
    """Create a minimal valid EPUB file."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as zf:
        zf.writestr("mimetype", "application/epub+zip", compress_type=zipfile.ZIP_STORED)
        zf.writestr(
            "META-INF/container.xml",
            """<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>""",
        )

        # Escape XML special characters
        safe_title = title.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
        safe_author = author.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
        safe_desc = description.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

        zf.writestr(
            "OEBPS/content.opf",
            f"""<?xml version="1.0"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>{safe_title}</dc:title>
    <dc:creator>{safe_author}</dc:creator>
    <dc:language>en</dc:language>
    <dc:description>{safe_desc}</dc:description>
    <dc:identifier id="BookId">urn:uuid:{uuid.uuid4()}</dc:identifier>
  </metadata>
  <manifest>
    <item id="content" href="content.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>""",
        )
        zf.writestr(
            "OEBPS/content.xhtml",
            f"""<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>{safe_title}</title></head>
  <body>
    <h1>{safe_title}</h1>
    <h2>by {safe_author}</h2>
    <p>{safe_desc}</p>
  </body>
</html>""",
        )
    return buffer.getvalue()


def request_json(method, url, token, payload, return_json=False):
    """Make a JSON request, optionally returning the parsed response."""
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    req = request.Request(url, data=body, method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with request.urlopen(req) as resp:
            if resp.status >= 400:
                raise RuntimeError(f"HTTP {resp.status} from {url}")
            if return_json:
                return json.loads(resp.read().decode("utf-8"))
            return None
    except HTTPError as e:
        raise RuntimeError(f"HTTP {e.code} from {url}: {e.read().decode('utf-8', errors='replace')}")


def request_binary(method, url, token, payload, content_type):
    req = request.Request(url, data=payload, method=method)
    req.add_header("Content-Type", content_type)
    req.add_header("Authorization", f"Bearer {token}")
    try:
        with request.urlopen(req) as resp:
            if resp.status >= 400:
                raise RuntimeError(f"HTTP {resp.status} from {url}")
    except HTTPError as e:
        raise RuntimeError(f"HTTP {e.code} from {url}: {e.read().decode('utf-8', errors='replace')}")


def get_admin_token(base_url, email, password):
    """Authenticate as admin and return the auth token."""
    payload = {"email": email, "password": password}
    result = request_json("POST", f"{base_url}/api/v1/admin/login", None, payload, return_json=True)
    return result["token"]


def generate_title(rng):
    """Generate a book title from patterns."""
    pattern = rng.choice(TITLE_PATTERNS)
    return pattern.format(
        adj=rng.choice(ADJECTIVES),
        noun=rng.choice(NOUNS),
        noun2=rng.choice(NOUNS),
        noun3=rng.choice(NOUNS),
        place=rng.choice(PLACES),
    )


def generate_series_name(rng):
    """Generate a series name."""
    pattern = rng.choice(SERIES_NAMES)
    return pattern.format(
        adj=rng.choice(ADJECTIVES),
        noun=rng.choice(NOUNS),
        place=rng.choice(PLACES),
    )


def generate_description(rng, genre):
    """Generate a book description."""
    template = rng.choice(DESCRIPTION_TEMPLATES)

    adj_list = list(ADJECTIVES)
    themes = list(THEMES)
    rng.shuffle(themes)

    return template.format(
        concept=rng.choice(CONCEPTS),
        protagonist=rng.choice(PROTAGONISTS),
        quest=rng.choice(QUESTS),
        threat=rng.choice(THREATS),
        adj=rng.choice(adj_list),
        adj2=rng.choice(adj_list),
        theme1=themes[0],
        theme2=themes[1],
        noun=rng.choice(NOUNS).lower(),
        noun2=rng.choice(NOUNS).lower(),
        discovery=f"discover {rng.choice(['their hidden powers', 'an ancient secret', 'a terrible truth', 'their true heritage'])}",
        event=rng.choice(EVENTS),
        place=rng.choice(PLACES),
        goal=rng.choice(QUESTS),
        companion=rng.choice(['a mysterious guide', 'an unlikely ally', 'a loyal friend', 'a former enemy']),
        antagonist=rng.choice(['a dark sorcerer', 'an ancient evil', 'a ruthless emperor', 'the forces of darkness']),
        mystery=rng.choice(['their past', 'the old kingdom', 'the ancient prophecy', 'their family']),
        profession=rng.choice(PROFESSIONS),
        choice1=rng.choice(['love', 'duty', 'honor', 'family']),
        choice2=rng.choice(['survival', 'vengeance', 'the greater good', 'their own heart']),
    )


def generate_book(index, rng, series_cache):
    """Generate a complete book with all metadata."""

    # Pick primary genre (weighted)
    genre_weights = [(g, info["weight"]) for g, info in GENRES.items()]
    total_weight = sum(w for _, w in genre_weights)
    r = rng.random() * total_weight
    cumulative = 0
    primary_genre = "Fantasy"
    for genre, weight in genre_weights:
        cumulative += weight
        if r <= cumulative:
            primary_genre = genre
            break

    # Build genre list (1-3 genres)
    genres = [primary_genre]
    if rng.random() < 0.6:
        subgenre = rng.choice(SUBGENRES.get(primary_genre, [primary_genre]))
        if subgenre not in genres:
            genres.append(subgenre)
    if rng.random() < 0.2:
        other_genre = rng.choice(list(GENRES.keys()))
        if other_genre not in genres:
            genres.append(other_genre)

    # Generate title
    title = generate_title(rng)

    # 65% chance of being in a series
    series_name = None
    series_index = None
    if rng.random() < 0.65:
        # Reuse existing series sometimes
        if series_cache and rng.random() < 0.7:
            series_name = rng.choice(list(series_cache.keys()))
            series_cache[series_name] += 1
            series_index = series_cache[series_name]
        else:
            series_name = generate_series_name(rng)
            series_index = 1
            series_cache[series_name] = 1

        series_index = float(series_index)
        # Occasionally add .5 entries
        if rng.random() < 0.1:
            series_index = series_index - 0.5

    # Generate author
    author = f"{rng.choice(AUTHOR_FIRST)} {rng.choice(AUTHOR_LAST)}"

    # Generate other metadata
    publisher = rng.choice(PUBLISHERS)

    # Publication date: weighted toward recent years
    years_ago = int(rng.expovariate(0.15))  # Exponential distribution, most books recent
    years_ago = min(years_ago, 50)
    pub_date = datetime.now() - timedelta(days=years_ago * 365 + rng.randint(0, 364))
    # RFC3339 format required by API
    pubdate = pub_date.strftime("%Y-%m-%dT00:00:00Z")

    # Page count: normal distribution around 350, min 150, max 1200
    page_count = int(max(150, min(1200, rng.gauss(350, 120))))

    # Rating: weighted toward higher ratings (most books are 3.5-4.5)
    rating = round(max(1.0, min(5.0, rng.gauss(3.9, 0.6))), 1)

    # Rating count: exponential distribution (few have many ratings)
    rating_count = int(rng.expovariate(0.0001))
    rating_count = max(10, min(500000, rating_count))

    # Description
    description = generate_description(rng, primary_genre)

    # Generate ISBN-like identifier
    isbn_prefix = rng.choice(["978", "979"])
    isbn = f"{isbn_prefix}-{rng.randint(0,9)}-{rng.randint(10000,99999)}-{rng.randint(100,999)}-{rng.randint(0,9)}"

    return {
        "title": title,
        "authors": [author],
        "series_name": series_name,
        "series_index": series_index,
        "language": "en",
        "genres": genres,
        "publisher": publisher,
        "pubdate": pubdate,
        "page_count": page_count,
        "rating": rating,
        "rating_count": rating_count,
        "description": description,
        "identifiers": {"isbn13": isbn.replace("-", "")},
        "primary_genre": primary_genre,  # For cover generation
    }


def seed_book(base, token, book):
    """Seed a single book to the API. Returns book metadata including author names and series name."""
    book_id = uuid.uuid5(uuid.NAMESPACE_URL, f"kobbler-demo:{book['title']}")

    payload = {
        "title": book["title"],
        "description": book.get("description"),
        "language": book["language"],
        "publisher": book.get("publisher"),
        "pubdate": book.get("pubdate"),
        "series_name": book["series_name"],
        "series_index": book["series_index"],
        "authors": book["authors"],
        "genres": book.get("genres"),
        "page_count": book.get("page_count"),
        "rating": book.get("rating"),
        "rating_count": book.get("rating_count"),
        "identifiers": book.get("identifiers"),
    }

    request_json("PUT", f"{base}/api/v1/books/{book_id}", token, payload)

    # Generate cover
    primary_genre = book.get("primary_genre", "Fantasy")
    cover_png = make_cover_png(
        book["title"],
        book["authors"][0],
        primary_genre,
        seed=hash(book["title"])
    )
    request_binary(
        "PUT",
        f"{base}/api/v1/books/{book_id}/cover",
        token,
        cover_png,
        "image/png",
    )

    # Create EPUB
    epub_bytes = make_epub_bytes(
        book["title"],
        book["authors"][0],
        book.get("description", "")
    )
    request_binary(
        "PUT",
        f"{base}/api/v1/books/{book_id}/files/KEPUB",
        token,
        epub_bytes,
        "application/octet-stream",
    )

    request_json("POST", f"{base}/api/v1/entitlements", token, {"book_id": str(book_id)})

    # Return metadata for tracking
    return {
        "book_id": str(book_id),
        "authors": book["authors"],
        "series_name": book["series_name"],
    }


def execute_sql(db_container, sql):
    """Execute SQL via psql in the DB container. Returns stdout on success."""
    result = subprocess.run(
        ["docker", "exec", db_container, "psql", "-U", "kobbler", "-d", "kobbler", "-t", "-c", sql],
        capture_output=True, text=True
    )
    if result.returncode != 0:
        print(f"SQL error: {result.stderr}", file=sys.stderr)
        return None
    return result.stdout.strip()


def execute_sql_batch(db_container, statements):
    """Execute multiple SQL statements in a single docker exec call via stdin.

    Much faster than calling execute_sql() per statement because each docker exec
    has ~0.5s overhead. For 5000 episodes this saves ~40 minutes.
    """
    if not statements:
        return True
    sql = "\n".join(statements)
    result = subprocess.run(
        ["docker", "exec", "-i", db_container, "psql", "-U", "kobbler", "-d", "kobbler"],
        input=sql, capture_output=True, text=True
    )
    if result.returncode != 0:
        print(f"SQL batch error: {result.stderr}", file=sys.stderr)
        return False
    return True


def get_entity_ids_by_name(base, admin_token, entity_type, names):
    """Fetch entity IDs from the API by name. Returns dict of name -> id."""
    name_to_id = {}

    if entity_type == "authors":
        # Fetch authors list and filter by name
        resp = request_json("GET", f"{base}/api/v1/library/authors?limit=1000", admin_token, None, return_json=True)
        for item in resp.get("items", []):
            if item["name"] in names:
                name_to_id[item["name"]] = item["id"]
    elif entity_type == "series":
        # Fetch series list and filter by name
        resp = request_json("GET", f"{base}/api/v1/library/series?limit=1000", admin_token, None, return_json=True)
        for item in resp.get("items", []):
            if item["name"] in names:
                name_to_id[item["name"]] = item["id"]

    return name_to_id


def seed_monitored_authors(base, admin_token, rng, author_ids):
    """Mark some authors as monitored with various auto_want_modes."""
    if not author_ids:
        return 0

    modes = ["none", "new_only", "all", "all"]  # Bias toward 'all' for more visible wanted books
    num = max(10, len(author_ids) // 2)  # Monitor ~50% of authors
    selected = rng.sample(list(author_ids.items()), min(num, len(author_ids)))

    count = 0
    for name, author_id in selected:
        mode = rng.choice(modes)
        try:
            request_json(
                "PUT",
                f"{base}/api/v1/library/authors/{author_id}/monitoring",
                admin_token,
                {"is_monitored": True, "auto_want_mode": mode}
            )
            count += 1
        except Exception as e:
            print(f"  Warning: Failed to set monitoring for author '{name}': {e}")

    return count


def seed_monitored_series(base, admin_token, rng, series_ids):
    """Mark some series as monitored with various auto_want_modes."""
    if not series_ids:
        return 0

    modes = ["inherit", "none", "new_only", "all", "fill_gaps"]
    num = max(2, len(series_ids) // 5)
    selected = rng.sample(list(series_ids.items()), min(num, len(series_ids)))

    count = 0
    for name, series_id in selected:
        mode = rng.choice(modes)
        try:
            request_json(
                "PUT",
                f"{base}/api/v1/library/series/{series_id}/monitoring",
                admin_token,
                {"is_monitored": True, "auto_want_mode": mode}
            )
            count += 1
        except Exception as e:
            print(f"  Warning: Failed to set monitoring for series '{name}': {e}")

    return count



def seed_wanted_books(base, ingest_token, db_container, rng, monitored_author_ids):
    """Create discovered/wanted books that are not in the library."""
    if not monitored_author_ids or not db_container:
        return 0

    # Generate some wanted book titles
    wanted_titles = [
        "The Undiscovered Realm",
        "Shadows of Tomorrow",
        "The Crimson Prophecy",
        "Echoes of the Fallen",
        "The Midnight Chronicler",
        "Whispers in the Dark",
        "The Shattered Crown",
        "Beyond the Veil of Stars",
        "The Last Enchantment",
        "Tides of Fortune",
        "The Silent Kingdom",
        "Daughters of the Storm",
        "The Forgotten Heir",
        "Ashes and Embers",
        "The Iron Testament",
        "The Burning Sky",
        "Crown of Shadows",
        "The Eternal Night",
        "Rivers of Gold",
        "The Glass Throne",
        "Winds of Change",
        "The Hollow King",
        "Starlight and Steel",
        "The Wanderer's Path",
        "Fire and Frost",
        "The Obsidian Gate",
        "Daughters of War",
        "The Serpent's Kiss",
        "Mountains of Madness",
        "The Silver Compass",
    ]

    statuses = ["pending", "wanted", "wanted", "wanted", "wanted", "skipped"]
    author_list = list(monitored_author_ids.items())

    count = 0
    for i, title in enumerate(wanted_titles):
        author_name, author_id = rng.choice(author_list)
        status = rng.choice(statuses)
        book_id = str(uuid.uuid4())

        # Generate a fake hardcover ID for identifiers
        hc_id = str(rng.randint(100000, 999999))

        sql = f"""
        INSERT INTO books (id, title, identifiers, is_in_library, want_status, discovered_from_author_id, first_seen_at, created_at, updated_at)
        VALUES (
            '{book_id}',
            '{title.replace("'", "''")}',
            '{{"hardcover": "{hc_id}"}}',
            FALSE,
            '{status}',
            '{author_id}',
            NOW() - INTERVAL '{rng.randint(1, 30)} days',
            NOW(),
            NOW()
        ) ON CONFLICT DO NOTHING;
        """

        result = execute_sql(db_container, sql)
        if result is not None:
            # Link the book to the author
            link_sql = f"""
            INSERT INTO book_authors (book_id, author_id)
            VALUES ('{book_id}', '{author_id}')
            ON CONFLICT DO NOTHING;
            """
            execute_sql(db_container, link_sql)

            # Generate and upload a cover image
            try:
                cover_png = make_cover_png(title, author_name, "Wanted", seed=hash(title))
                request_binary(
                    "PUT",
                    f"{base}/api/v1/books/{book_id}/cover",
                    ingest_token,
                    cover_png,
                    "image/png",
                )
            except Exception as e:
                pass  # Cover upload is optional

            count += 1

    return count


def seed_series_gaps(base, ingest_token, db_container, rng, series_ids, author_ids):
    """Add missing/wanted books to existing series to create gaps."""
    if not series_ids or not db_container:
        return 0

    # Pick some series to add gaps to
    num_series = min(20, len(series_ids) // 3)
    selected_series = rng.sample(list(series_ids.items()), num_series)

    count = 0
    for series_name, series_id in selected_series:
        # Get current max index in series
        max_idx_sql = f"""
        SELECT COALESCE(MAX(series_index), 0) FROM books WHERE series_id = '{series_id}';
        """
        result = execute_sql(db_container, max_idx_sql)
        try:
            max_idx = int(result.strip()) if result and result.strip() else 0
        except:
            max_idx = 0

        # Add 1-3 "missing" books after the current max
        num_missing = rng.randint(1, 3)
        for j in range(num_missing):
            book_idx = max_idx + j + 1
            title = generate_title(rng)
            status = rng.choice(["wanted", "wanted", "pending", "skipped"])
            book_id = str(uuid.uuid4())
            hc_id = str(rng.randint(100000, 999999))

            # Pick a random author from the pool
            author_name = rng.choice(list(author_ids.keys())) if author_ids else "Unknown Author"
            author_id = author_ids.get(author_name)

            sql = f"""
            INSERT INTO books (id, title, series_id, series_index, identifiers, is_in_library, want_status, first_seen_at, created_at, updated_at)
            VALUES (
                '{book_id}',
                '{title.replace("'", "''")}',
                '{series_id}',
                {float(book_idx)},
                '{{"hardcover": "{hc_id}"}}',
                FALSE,
                '{status}',
                NOW() - INTERVAL '{rng.randint(1, 14)} days',
                NOW(),
                NOW()
            ) ON CONFLICT DO NOTHING;
            """

            result = execute_sql(db_container, sql)
            if result is not None:
                # Link to author if we have one
                if author_id:
                    link_sql = f"""
                    INSERT INTO book_authors (book_id, author_id)
                    VALUES ('{book_id}', '{author_id}')
                    ON CONFLICT DO NOTHING;
                    """
                    execute_sql(db_container, link_sql)

                # Generate cover
                try:
                    cover_png = make_cover_png(title, author_name, "Wanted", seed=hash(title))
                    request_binary(
                        "PUT",
                        f"{base}/api/v1/books/{book_id}/cover",
                        ingest_token,
                        cover_png,
                        "image/png",
                    )
                except:
                    pass

                count += 1

    return count


def seed_ingest_queue(db_container):
    """Create ingest queue items in various statuses.

    Since migration 0030, ingest items are stored in books table with ingest_status column,
    plus book_ingest_source for file-related metadata.
    """
    if not db_container:
        return 0

    # Note: 'approved' status is represented by ingest_status=NULL (item is now a library book)
    # We skip creating approved items here since they'd just be regular books
    items = [
        ("pending", "Pending Book.epub", "The Pending Book", "Demo Author", None),
        ("pending", "Another Pending.epub", "Another Pending", "Jane Writer", None),
        ("extracting", "Extracting.epub", "Extracting Book", None, None),  # In progress
        ("error", "Error Book.epub", "Failed Book", "Error Author", "EPUB parsing failed: corrupted file"),
        ("rejected", "Rejected.epub", "Unwanted Book", "Skip Author", None),
    ]

    count = 0
    for status, filename, title, author, error in items:
        item_id = str(uuid.uuid4())
        title_value = title if title else filename
        title_sql = f"'{title_value.replace(chr(39), chr(39)+chr(39))}'"
        error_sql = f"'{error.replace(chr(39), chr(39)+chr(39))}'" if error else "NULL"
        # Store author as pending_authors JSONB if provided
        pending_authors_sql = f"'[{{\"name\": \"{author}\"}}]'::jsonb" if author else "NULL"

        # Insert into books table with ingest_status
        books_sql = f"""
        INSERT INTO books (
            id, title, is_in_library, want_status, ingest_status, created_at, updated_at
        ) VALUES (
            '{item_id}',
            {title_sql},
            FALSE,
            'none',
            '{status}',
            NOW() - INTERVAL '{6 - count} hours',
            NOW()
        ) ON CONFLICT DO NOTHING;
        """

        result = execute_sql(db_container, books_sql)
        if result is None:
            continue

        # Insert into book_ingest_source table
        source_sql = f"""
        INSERT INTO book_ingest_source (
            book_id, file_path, file_name, file_size, source_format,
            pending_authors, conversion_error, created_at
        ) VALUES (
            '{item_id}',
            '/demo/uploads/{filename}',
            '{filename}',
            {1024 * 1024 + count * 100000},
            'EPUB',
            {pending_authors_sql},
            {error_sql},
            NOW() - INTERVAL '{6 - count} hours'
        ) ON CONFLICT DO NOTHING;
        """

        result = execute_sql(db_container, source_sql)
        if result is not None:
            count += 1

    return count


def seed_people_and_credits(db_container, rng, movie_results, tv_show_count):
    """Generate a pool of fake people and assign them as cast/crew to movies and TV shows."""
    if not db_container:
        return {"count": 0, "people": []}

    # Generate a pool of people
    people = []
    num_people = 80
    for i in range(num_people):
        person_id = str(uuid.uuid4())
        first = rng.choice(ACTOR_FIRST)
        last = rng.choice(ACTOR_LAST)
        name = f"{first} {last}"
        sort_name = f"{last}, {first}"
        tmdb_id = str(100000 + i)
        people.append({"id": person_id, "name": name, "sort_name": sort_name, "tmdb_id": tmdb_id})

    # Insert all people
    statements = []
    for p in people:
        name_sql = p["name"].replace("'", "''")
        sort_sql = p["sort_name"].replace("'", "''")
        statements.append(
            f"""INSERT INTO people (id, name, sort_name, identifiers, created_at, updated_at)
            VALUES ('{p["id"]}', '{name_sql}', '{sort_sql}',
                    '{{"tmdb_id": "{p["tmdb_id"]}"}}'::jsonb, NOW(), NOW())
            ON CONFLICT DO NOTHING;"""
        )
    execute_sql_batch(db_container, statements)

    # Assign cast and crew to movies
    statements = []
    for movie in movie_results:
        movie_id = movie["id"]
        # Pick 3-8 cast members
        num_cast = rng.randint(3, 8)
        cast_pool = rng.sample(people, min(num_cast, len(people)))
        for order, person in enumerate(cast_pool):
            char_first = rng.choice(CHARACTER_FIRST)
            char_last = rng.choice(CHARACTER_LAST)
            character = f"{char_first} {char_last}"
            cast_id = str(uuid.uuid4())
            statements.append(
                f"""INSERT INTO movie_cast (id, movie_id, person_id, character_name, cast_order)
                VALUES ('{cast_id}', '{movie_id}', '{person["id"]}',
                        '{character}', {order})
                ON CONFLICT DO NOTHING;"""
            )

        # Pick 2-5 crew members (director + others)
        crew_pool = rng.sample(people, min(5, len(people)))
        # Always have a director
        director_person = crew_pool[0]
        statements.append(
            f"""INSERT INTO movie_crew (movie_id, person_id, department, job)
            VALUES ('{movie_id}', '{director_person["id"]}', 'Directing', 'Director')
            ON CONFLICT DO NOTHING;"""
        )
        for person in crew_pool[1:]:
            dept = rng.choice(list(JOBS_BY_DEPT.keys()))
            job = rng.choice(JOBS_BY_DEPT[dept])
            job_sql = job.replace("'", "''")
            statements.append(
                f"""INSERT INTO movie_crew (movie_id, person_id, department, job)
                VALUES ('{movie_id}', '{person["id"]}', '{dept}', '{job_sql}')
                ON CONFLICT DO NOTHING;"""
            )

    if statements:
        execute_sql_batch(db_container, statements)
    movie_credit_count = len(statements)

    # Assign cast and crew to TV shows — need show IDs from DB
    tv_ids_raw = execute_sql(db_container, "SELECT id FROM tv_shows;")
    if not tv_ids_raw:
        return movie_credit_count

    tv_ids = [line.strip() for line in tv_ids_raw.strip().split("\n") if line.strip()]

    statements = []
    for show_id in tv_ids:
        # Pick 4-10 cast members
        num_cast = rng.randint(4, 10)
        cast_pool = rng.sample(people, min(num_cast, len(people)))
        for order, person in enumerate(cast_pool):
            char_first = rng.choice(CHARACTER_FIRST)
            char_last = rng.choice(CHARACTER_LAST)
            character = f"{char_first} {char_last}"
            cast_id = str(uuid.uuid4())
            statements.append(
                f"""INSERT INTO tv_cast (id, show_id, person_id, character_name, cast_order)
                VALUES ('{cast_id}', '{show_id}', '{person["id"]}',
                        '{character}', {order})
                ON CONFLICT DO NOTHING;"""
            )

        # Pick 2-4 crew members
        crew_pool = rng.sample(people, min(4, len(people)))
        for person in crew_pool:
            dept = rng.choice(list(JOBS_BY_DEPT.keys()))
            job = rng.choice(JOBS_BY_DEPT[dept])
            job_sql = job.replace("'", "''")
            statements.append(
                f"""INSERT INTO tv_crew (show_id, person_id, department, job)
                VALUES ('{show_id}', '{person["id"]}', '{dept}', '{job_sql}')
                ON CONFLICT DO NOTHING;"""
            )

    if statements:
        execute_sql_batch(db_container, statements)

    return {"count": len(people), "people": people}


def trigger_search_reindex(base, admin_token):
    """Trigger a full search reindex to pick up any direct SQL inserts."""
    try:
        request_json("POST", f"{base}/api/v1/admin/search/reindex", admin_token, {}, return_json=True)
        return True
    except Exception as e:
        print(f"  Warning: Failed to trigger search reindex: {e}")
        return False


def seed_proxy_logs(db_container, device_id):
    """Create diverse proxy log entries."""
    if not db_container:
        return 0

    device_id_sql = f"'{device_id}'" if device_id else "NULL"

    # (method, path, decision, status, severity, duration_ms, notes)
    logs = [
        ("GET", "/v1/library/sync", "intercept", 200, "info", 45, None),
        ("PUT", "/v1/library/annotations", "intercept", 200, "info", 120, None),
        ("GET", "/v1/user/profile", "merge", 200, "info", 85, None),
        ("GET", "/v1/products/12345", "passthrough", 200, "info", 230, None),
        ("GET", "/v1/library/tags", "intercept", 200, "info", 35, None),
        ("PUT", "/v1/reading/state", "intercept", 200, "info", 95, None),
        ("GET", "/v1/initialization", "merge", 200, "info", 150, None),
        ("GET", "/v1/library/books", "intercept", 200, "info", 180, None),
        ("GET", "/v1/products/missing", "passthrough", 404, "warning", 180, "Product not found upstream"),
        ("GET", "/v1/library/slow-query", "intercept", 200, "warning", 1500, "Slow database query"),
        ("POST", "/v1/broken-endpoint", "intercept", 500, "error", 5, "Handler panic: index out of bounds"),
        ("PUT", "/v1/upstream-timeout", "passthrough", 504, "error", 30000, "Upstream timeout after 30s"),
    ]

    count = 0
    for i, (method, path, decision, status, severity, duration, notes) in enumerate(logs):
        log_id = str(uuid.uuid4())
        notes_sql = f"'{notes.replace(chr(39), chr(39)+chr(39))}'" if notes else "NULL"

        sql = f"""
        INSERT INTO proxy_logs (
            id, device_id, request_method, request_path, decision,
            response_status, severity, total_duration_ms, notes, status, created_at
        ) VALUES (
            '{log_id}',
            {device_id_sql},
            '{method}',
            '{path}',
            '{decision}',
            {status},
            '{severity}',
            {duration},
            {notes_sql},
            'complete',
            NOW() - INTERVAL '{len(logs) - i} minutes'
        ) ON CONFLICT DO NOTHING;
        """

        result = execute_sql(db_container, sql)
        if result is not None:
            count += 1

    return count


def main():
    parser = argparse.ArgumentParser(description="Seed demo books with realistic data")
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", required=True, help="Ingest token")
    parser.add_argument("--admin-email", help="Admin email for authenticated operations")
    parser.add_argument("--admin-password", help="Admin password")
    parser.add_argument("--db-container", help="Docker DB container name for direct SQL")
    parser.add_argument("--device-id", help="Device ID for proxy logs")
    parser.add_argument("--count", type=int, default=100,
                       help="Number of books to generate (default: 100)")
    parser.add_argument("--seed", type=int, default=42,
                       help="Random seed for reproducible generation")
    args = parser.parse_args()

    base = args.base_url.rstrip("/")
    token = args.token.strip()

    rng = random.Random(args.seed)
    series_cache = {}  # Track series for consistent numbering

    # Track created entities for later seeding
    all_authors = set()
    all_series = set()

    if args.count > 0:
        print(f"Generating {args.count} books...")

        # Pre-generate all books (fast, uses RNG so must be sequential)
        books = [generate_book(i + 1, rng, series_cache) for i in range(args.count)]

        # Seed books in parallel
        max_workers = min(16, args.count)  # Limit concurrent requests
        completed = 0
        failed = 0

        def seed_one(book):
            """Seed a single book, return result or None on failure."""
            try:
                return seed_book(base, token, book)
            except Exception as e:
                return None

        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = {executor.submit(seed_one, book): book for book in books}

            for future in as_completed(futures):
                result = future.result()
                if result:
                    for author in result["authors"]:
                        all_authors.add(author)
                    if result["series_name"]:
                        all_series.add(result["series_name"])
                    completed += 1
                else:
                    failed += 1

                total_done = completed + failed
                if total_done % 50 == 0 or total_done == args.count:
                    print(f"  Seeded {completed}/{args.count} books ({failed} failed)...")

        print(f"Seeded {completed} books across {len(series_cache)} series.")

    # Seed additional data if admin credentials are provided
    if args.admin_email and args.admin_password:
        print("\nSeeding additional demo data...")
        try:
            admin_token = get_admin_token(base, args.admin_email, args.admin_password)
        except Exception as e:
            print(f"  Warning: Admin login failed, skipping additional seeding: {e}")
            return

        # Get entity IDs from API
        author_ids = get_entity_ids_by_name(base, admin_token, "authors", all_authors)
        series_ids = get_entity_ids_by_name(base, admin_token, "series", all_series)

        # Seed monitored authors
        monitored_count = seed_monitored_authors(base, admin_token, rng, author_ids)
        print(f"  Marked {monitored_count} authors as monitored")

        # Get list of monitored author IDs for wanted books
        monitored_author_ids = {}
        try:
            resp = request_json("GET", f"{base}/api/v1/library/authors?is_monitored=true&limit=100", admin_token, None, return_json=True)
            for item in resp.get("items", []):
                monitored_author_ids[item["name"]] = item["id"]
        except Exception:
            pass

        # Seed monitored series
        monitored_series_count = seed_monitored_series(base, admin_token, rng, series_ids)
        print(f"  Marked {monitored_series_count} series as monitored")


        # Direct DB operations require container name
        if args.db_container:
            # Seed wanted/discovered books
            wanted_count = seed_wanted_books(base, token, args.db_container, rng, monitored_author_ids)
            print(f"  Created {wanted_count} discovered/wanted books")

            # Seed series gaps (missing books in series)
            gaps_count = seed_series_gaps(base, token, args.db_container, rng, series_ids, author_ids)
            print(f"  Created {gaps_count} missing books in series (gaps)")

            # Seed movies
            movie_count = max(5, args.count * 2 // 5)
            movie_results = seed_movies(args.db_container, rng, movie_count)
            print(f"  Created {len(movie_results)} movies")

            # Upload movie posters and backdrops
            movie_img_count = 0
            for m in movie_results:
                try:
                    genre = rng.choice(list(MOVIE_GENRES.keys()))
                    poster = make_poster_png(m["title"], genre, seed=hash(m["title"]))
                    request_binary(
                        "POST",
                        f"{base}/api/v1/admin/movies/{m['id']}/poster",
                        admin_token,
                        poster,
                        "image/png",
                    )
                    backdrop = make_backdrop_png(m["title"], genre, seed=hash(m["title"]) + 1)
                    request_binary(
                        "POST",
                        f"{base}/api/v1/admin/movies/{m['id']}/backdrop",
                        admin_token,
                        backdrop,
                        "image/png",
                    )
                    movie_img_count += 1
                except Exception as e:
                    print(f"  Warning: failed to upload images for movie {m['title']}: {e}")
            print(f"  Uploaded posters/backdrops for {movie_img_count} movies")

            # Seed TV shows (includes stress-test shows for large counts)
            tv_count = max(3, args.count // 5)
            tv_results = seed_tv_shows(args.db_container, rng, tv_count)
            print(f"  Created {tv_results['show_count']} TV shows with {tv_results['episode_count']} episodes")

            # Upload TV show posters and backdrops
            tv_img_count = 0
            for s in tv_results.get("shows", []):
                try:
                    genre = rng.choice(list(MOVIE_GENRES.keys()))
                    poster = make_poster_png(s["title"], genre, seed=hash(s["title"]))
                    request_binary(
                        "POST",
                        f"{base}/api/v1/admin/tv/shows/{s['id']}/poster",
                        admin_token,
                        poster,
                        "image/png",
                    )
                    backdrop = make_backdrop_png(s["title"], genre, seed=hash(s["title"]) + 1)
                    request_binary(
                        "POST",
                        f"{base}/api/v1/admin/tv/shows/{s['id']}/backdrop",
                        admin_token,
                        backdrop,
                        "image/png",
                    )
                    tv_img_count += 1
                except Exception as e:
                    print(f"  Warning: failed to upload images for show {s['title']}: {e}")
            print(f"  Uploaded posters/backdrops for {tv_img_count} TV shows")

            # Upload season posters
            season_poster_count = 0
            for s in tv_results.get("shows", []):
                for season in s.get("seasons", []):
                    try:
                        poster = make_poster_png(
                            f"{s['title']} S{season['number']}",
                            rng.choice(list(MOVIE_GENRES.keys())),
                            seed=hash(f"{s['title']}_s{season['number']}")
                        )
                        request_binary(
                            "POST",
                            f"{base}/api/v1/admin/tv/seasons/{season['id']}/poster",
                            admin_token,
                            poster,
                            "image/png",
                        )
                        season_poster_count += 1
                    except Exception as e:
                        pass  # Silently skip failed uploads
            print(f"  Uploaded {season_poster_count} season posters")

            # Seed people and credits for movies/TV
            people_result = seed_people_and_credits(
                args.db_container, rng, movie_results, tv_count
            )
            print(f"  Created {people_result['count']} people with cast/crew credits")

            # Upload person images
            person_img_count = 0
            for p in people_result.get("people", []):
                try:
                    img = make_poster_png(
                        p["name"], "Drama",
                        width=200, height=200,
                        seed=hash(p["name"])
                    )
                    request_binary(
                        "POST",
                        f"{base}/api/v1/admin/people/{p['id']}/image",
                        admin_token,
                        img,
                        "image/png",
                    )
                    person_img_count += 1
                except Exception as e:
                    pass
            print(f"  Uploaded {person_img_count} person images")

            # Seed ingest queue
            queue_count = seed_ingest_queue(args.db_container)
            print(f"  Created {queue_count} ingest queue items")

            # Seed proxy logs
            log_count = seed_proxy_logs(args.db_container, args.device_id)
            print(f"  Created {log_count} proxy log entries")

            # Trigger search reindex to pick up direct SQL inserts
            if wanted_count > 0 or gaps_count > 0 or movie_results or tv_results['episode_count'] > 0:
                if trigger_search_reindex(base, admin_token):
                    print("  Triggered search reindex")
        else:
            print("  Skipping DB-direct seeding (no --db-container specified)")

    print("\nDone!")


if __name__ == "__main__":
    main()
