import sqlite3
from pathlib import Path

SOURCE = "/home/ubuntu/python-stats-import.db"
TARGET = "/home/ubuntu/cyberpup-bot-rust/data/stats.db"


def normalize(value):
    return (value or "").strip().casefold()


def main():
    if not Path(SOURCE).exists():
        raise FileNotFoundError(SOURCE)

    if not Path(TARGET).exists():
        raise FileNotFoundError(TARGET)

    db = sqlite3.connect(TARGET)
    db.row_factory = sqlite3.Row

    try:
        db.execute("ATTACH DATABASE ? AS legacy", (SOURCE,))
        db.execute("BEGIN IMMEDIATE")

        legacy_creators = db.execute("""
            SELECT id, name, youtube_id, twitch_username,
                   discord_invite_code
            FROM legacy.creators
            ORDER BY id
        """).fetchall()

        rust_creators = db.execute("""
            SELECT id, name, youtube_id, twitch_username
            FROM main.creators
        """).fetchall()

        mappings = []

        print("\nMapping creators:")

        for creator in legacy_creators:
            twitch = normalize(creator["twitch_username"])
            youtube = normalize(creator["youtube_id"])

            matches = set()

            for existing in rust_creators:
                if twitch and twitch == normalize(existing["twitch_username"]):
                    matches.add(existing["id"])

                if youtube and youtube == normalize(existing["youtube_id"]):
                    matches.add(existing["id"])

            if len(matches) > 1:
                raise RuntimeError(
                    f"Ambiguous match for {creator['name']}: {matches}"
                )

            if matches:
                rust_id = matches.pop()
                print(
                    f"  MATCH: {creator['name']} "
                    f"(Python {creator['id']} -> Rust {rust_id})"
                )
            else:
                cursor = db.execute("""
                    INSERT INTO main.creators (
                        name,
                        youtube_id,
                        twitch_username,
                        discord_invite_code
                    )
                    VALUES (?, ?, ?, ?)
                """, (
                    creator["name"],
                    creator["youtube_id"],
                    creator["twitch_username"],
                    creator["discord_invite_code"],
                ))

                rust_id = cursor.lastrowid

                rust_creators.append({
                    "id": rust_id,
                    "name": creator["name"],
                    "youtube_id": creator["youtube_id"],
                    "twitch_username": creator["twitch_username"],
                })

                print(
                    f"  CREATED: {creator['name']} "
                    f"(Python {creator['id']} -> Rust {rust_id})"
                )

            mappings.append((creator["id"], rust_id))

        db.execute("""
            CREATE TEMP TABLE migration_map (
                python_id INTEGER PRIMARY KEY,
                rust_id INTEGER NOT NULL
            )
        """)

        db.executemany("""
            INSERT INTO migration_map (python_id, rust_id)
            VALUES (?, ?)
        """, mappings)

        before = db.execute(
            "SELECT COUNT(*) FROM main.metrics"
        ).fetchone()[0]

        print(f"\nExisting Rust snapshots: {before:,}")
        print("Importing historical metrics...")

        cursor = db.execute("""
            INSERT INTO main.metrics (
                timestamp,
                creator_id,
                yt_subs,
                twitch_followers,
                discord_members,
                is_live,
                viewers,
                stream_title,
                thumbnail_url,
                game_name
            )
            SELECT
                src.timestamp,
                mapping.rust_id,
                src.yt_subs,
                src.twitch_followers,
                src.discord_members,
                src.is_live,
                src.viewers,
                src.stream_title,
                src.thumbnail_url,
                src.game_name
            FROM legacy.metrics AS src
            JOIN migration_map AS mapping
                ON mapping.python_id = src.creator_id
            WHERE NOT EXISTS (
                SELECT 1
                FROM main.metrics AS existing
                WHERE existing.creator_id = mapping.rust_id
                  AND existing.timestamp IS src.timestamp
                  AND existing.yt_subs IS src.yt_subs
                  AND existing.twitch_followers IS src.twitch_followers
                  AND existing.discord_members IS src.discord_members
                  AND existing.is_live IS src.is_live
                  AND existing.viewers IS src.viewers
                  AND existing.stream_title IS src.stream_title
                  AND existing.thumbnail_url IS src.thumbnail_url
                  AND existing.game_name IS src.game_name
            )
        """)

        imported = cursor.rowcount

        after = db.execute(
            "SELECT COUNT(*) FROM main.metrics"
        ).fetchone()[0]

        db.commit()

        print("\nMigration completed!")
        print(f"Creators mapped: {len(mappings)}")
        print(f"New metric snapshots: {imported:,}")
        print(f"Total Rust snapshots: {after:,}")
        print(f"Previously existing: {before:,}")

    except Exception:
        db.rollback()
        raise

    finally:
        db.close()


if __name__ == "__main__":
    main()