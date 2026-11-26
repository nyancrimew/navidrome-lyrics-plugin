use crate::cache::{CacheLookup, LyricsCache};
use crate::config::PluginConfig;
use crate::ext::TrackInfoExt;
use crate::fetch::{self, Outcome};
use crate::providers::ExtraTrackInfo;
use crate::types::{Lyrics, LyricsKind};
use crate::writing;
use extism_pdk::{debug, info, warn};
use nd_pdk::host;
use nd_pdk::host::matcher::MatchOptions;
use nd_pdk::lyrics::{Error as LyricsError, GetLyricsResponse, TrackInfo};
use nd_pdk::types::SongRef;
use std::rc::Rc;

pub fn get_lyrics(track: TrackInfo) -> Result<GetLyricsResponse, LyricsError> {
    let cfg = Rc::new(PluginConfig::load()?);
    let cache = cfg.enable_cache.then(|| LyricsCache::new(cfg.clone()));

    let lyrics = match cache_lookup(cache.as_ref(), &track) {
        Some(lyrics) => lyrics,
        None => {
            let lyrics = fetch_fresh(&track, &cfg, cache.as_ref())?;
            cache_save(cache.as_ref(), &track, &lyrics, &cfg);
            lyrics
        }
    };

    if cfg.skips_instrumental() && lyrics.kind() == LyricsKind::Instrumental {
        info!("'{}' is instrumental, not writing lyrics", track.label());
        return Err(LyricsError::new(format!(
            "'{}' is an instrumental track",
            track.label()
        )));
    }

    write_if_enabled(&track, &lyrics, &cfg);
    Ok(lyrics.to_response(&cfg))
}

fn fetch_fresh(
    track: &TrackInfo,
    cfg: &PluginConfig,
    cache: Option<&LyricsCache>,
) -> Result<Lyrics, LyricsError> {
    let extra = fetch_extra_info(track)?;
    match fetch::run(track, &extra, cfg, cache) {
        Outcome::Found(lyrics) => Ok(lyrics),
        Outcome::NotFound => Err(LyricsError::new(format!(
            "no lyrics found for '{}' from any provider",
            track.label()
        ))),
        Outcome::ProviderError => Err(LyricsError::new(format!(
            "all providers errored while fetching lyrics for '{}'",
            track.label()
        ))),
    }
}

fn fetch_extra_info(track: &TrackInfo) -> Result<ExtraTrackInfo, LyricsError> {
    let search = vec![SongRef {
        id: track.clone().id,
        ..Default::default()
    }];
    let tracks = host::matcher::match_songs(search, MatchOptions::default()).map_err(|e| {
        LyricsError::new(format!(
            "failed to fetch extra info for '{}': {e}",
            track.label()
        ))
    })?;
    if !tracks.is_empty() {
        if let Some(Some(matched_track)) = tracks.first() {
            let isrc = matched_track.tags.get("isrc").and_then(|f| f.first());
            let barcode = matched_track.tags.get("barcode").and_then(|f| f.first());

            return Ok(ExtraTrackInfo {
                isrc: isrc.cloned(),
                barcode: barcode.cloned(),
            });
        }
    }
    Ok(Default::default())
}

fn cache_lookup(cache: Option<&LyricsCache>, track: &TrackInfo) -> Option<Lyrics> {
    let Some(cache) = cache else {
        debug!("cache disabled");
        return None;
    };

    match cache.lookup(&track.id) {
        CacheLookup::Found(lyrics) => {
            info!(
                "cache hit ({}) for '{}'",
                lyrics.kind().slug(),
                track.label()
            );
            Some(lyrics)
        }
        CacheLookup::Miss => {
            info!("cache miss for '{}'", track.label());
            None
        }
    }
}

fn cache_save(cache: Option<&LyricsCache>, track: &TrackInfo, lyrics: &Lyrics, cfg: &PluginConfig) {
    let Some(cache) = cache else {
        return;
    };

    match cache.write(&track.id, lyrics) {
        Ok(()) => info!(
            "cached {} lyrics for track '{}' (ttl {}h)",
            lyrics.kind().slug(),
            track.label(),
            cfg.cache_ttl_hours_for(lyrics.kind())
        ),
        Err(err) => warn!("failed to persist lyrics to cache: {err}"),
    }
}

fn write_if_enabled(track: &TrackInfo, lyrics: &Lyrics, cfg: &PluginConfig) {
    if cfg.write_lyrics
        && let Err(err) = writing::write(track, lyrics, cfg)
    {
        warn!("failed to write lyrics file to disk: {err}");
    }
}
