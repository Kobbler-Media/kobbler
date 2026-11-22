//! A usenet season pack is one release, one `release_key`, holding a file per episode.
//! kobload picks the episode's file from the episode kobbler names and says which it
//! picked, so every session, cache file and analysis row is that file's alone.

use crate::common::debrid_rig::*;
use crate::common::usenet_pack::pack;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn pack_episodes_played_in_turn_each_get_their_own_file() {
    let pack = pack().await;

    let e05 = pack.tap(&pack.e05).await;
    let e05_path = pack.assert_played(&pack.e05, e05).await;
    let e06 = pack.tap(&pack.e06).await;
    assert_ne!(e06, e05, "E06 was handed E05's live session");
    let e06_path = pack.assert_played(&pack.e06, e06).await;

    assert_ne!(e05_path, e06_path);
    assert_eq!(pack.analysed(&e05_path).await, 1);
    assert_eq!(pack.analysed(&e06_path).await, 1);
    assert_eq!(
        pack.asked_for().await,
        [
            json!({ "season": 1, "episode": 5 }),
            json!({ "season": 1, "episode": 6 })
        ]
    );

    let again = pack.tap(&pack.e05).await;
    assert_eq!(again, e05, "a second tap of E05 opened another session");

    end_playback(&pack.rig.server, e05).await;
    let replay = pack.tap(&pack.e05).await;
    assert_ne!(replay, e05);
    assert_eq!(
        pack.cache_path(replay).await,
        e05_path,
        "the replay did not find E05's file"
    );
    let played = play(&pack.rig.server, replay, "bytes=0-").await;
    assert!(
        played.body == pack.e05.data,
        "the replay played another file"
    );
    assert_eq!(pack.analysed(&e05_path).await, 1, "one file, one analysis");
}

#[tokio::test]
async fn pack_episodes_played_at_once_each_get_their_own_file() {
    let pack = pack().await;

    let (e05, e06, e05_again) = tokio::join!(
        pack.tap(&pack.e05),
        pack.tap(&pack.e06),
        pack.tap(&pack.e05)
    );

    assert_ne!(e05, e06);
    assert_eq!(e05_again, e05, "two taps of E05 opened two sessions");
    let e05_path = pack.assert_played(&pack.e05, e05).await;
    let e06_path = pack.assert_played(&pack.e06, e06).await;
    assert_ne!(e05_path, e06_path);
}
