//! Decoded-frame cache.
//!
//! Decoding is expensive and playback is mostly sequential with occasional
//! jumps, so a modest cache in front of the decoder removes most repeated work.
//! The cache is bounded **by bytes rather than by entry count**, because a 4K
//! frame is roughly a thousand times the size of an audio packet and counting
//! entries would let a handful of frames exhaust memory.
//!
//! Eviction is least-recently-used. That is the right default for scrubbing and
//! for looping a range, which are the two access patterns that dominate.
//!
//! **The cache key includes the media's content hash**, so replacing a file
//! cannot serve stale frames from the previous one: a different file is a
//! different key, and the old entries simply age out.

use crate::frame::Frame;
use crate::identity::ContentHash;
use crate::time::Timestamp;
use indexmap::IndexMap;
use parking_lot::Mutex;
use std::sync::Arc;

/// Identifies one decoded frame unambiguously across the whole application.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameKey {
    /// Content hash of the source media, so identity survives a file move and
    /// a replaced file can never collide with the original.
    pub media_hash: String,
    pub stream: u32,
    /// Presentation time in the stream's own base.
    pub ticks: i64,
}

impl FrameKey {
    pub fn new(hash: &ContentHash, stream: u32, pts: Timestamp) -> Self {
        FrameKey {
            media_hash: hash.hex().to_string(),
            stream,
            ticks: pts.ticks(),
        }
    }
}

/// Cache statistics, exposed so behaviour can be measured rather than assumed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub bytes: usize,
    pub entries: usize,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// A byte-bounded LRU cache of decoded frames.
///
/// `IndexMap` gives insertion order, which is what makes LRU cheap here: a hit
/// moves the entry to the end, and eviction always takes from the front.
pub struct FrameCache {
    inner: Mutex<Inner>,
    capacity_bytes: usize,
}

struct Inner {
    map: IndexMap<FrameKey, Arc<Frame>>,
    bytes: usize,
    stats: CacheStats,
}

impl FrameCache {
    /// A cache holding at most `capacity_bytes` of decoded frames.
    pub fn new(capacity_bytes: usize) -> Self {
        FrameCache {
            inner: Mutex::new(Inner {
                map: IndexMap::new(),
                bytes: 0,
                stats: CacheStats::default(),
            }),
            capacity_bytes,
        }
    }

    /// 256 MiB, a reasonable default for editorial on a workstation.
    pub fn with_default_capacity() -> Self {
        FrameCache::new(256 * 1024 * 1024)
    }

    pub fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    pub fn get(&self, key: &FrameKey) -> Option<Arc<Frame>> {
        let mut inner = self.inner.lock();
        match inner.map.shift_remove(key) {
            Some(frame) => {
                // Re-insert at the end: most recently used.
                inner.map.insert(key.clone(), frame.clone());
                inner.stats.hits += 1;
                Some(frame)
            }
            None => {
                inner.stats.misses += 1;
                None
            }
        }
    }

    /// Insert a frame, evicting least-recently-used entries to stay in budget.
    ///
    /// A frame larger than the whole budget is not cached at all rather than
    /// evicting everything to make room for something that cannot help.
    pub fn insert(&self, key: FrameKey, frame: Arc<Frame>) {
        let size = frame.byte_size();
        if size > self.capacity_bytes {
            return;
        }
        let mut inner = self.inner.lock();
        if let Some(old) = inner.map.shift_remove(&key) {
            inner.bytes -= old.byte_size();
        }
        while inner.bytes + size > self.capacity_bytes && !inner.map.is_empty() {
            if let Some((_, evicted)) = inner.map.shift_remove_index(0) {
                inner.bytes -= evicted.byte_size();
                inner.stats.evictions += 1;
            }
        }
        inner.bytes += size;
        inner.map.insert(key, frame);
        let (b, n) = (inner.bytes, inner.map.len());
        inner.stats.bytes = b;
        inner.stats.entries = n;
    }

    /// Drop every frame belonging to one media file.
    ///
    /// Called when media is replaced or relinked, so nothing downstream can be
    /// served a frame from a file that no longer backs the clip.
    pub fn invalidate_media(&self, hash: &str) -> usize {
        let mut inner = self.inner.lock();
        let doomed: Vec<FrameKey> = inner
            .map
            .keys()
            .filter(|k| k.media_hash == hash)
            .cloned()
            .collect();
        let mut freed = 0;
        for k in &doomed {
            if let Some(f) = inner.map.shift_remove(k) {
                inner.bytes -= f.byte_size();
                freed += 1;
            }
        }
        let (b, n) = (inner.bytes, inner.map.len());
        inner.stats.bytes = b;
        inner.stats.entries = n;
        freed
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.bytes = 0;
        inner.stats.bytes = 0;
        inner.stats.entries = 0;
    }

    pub fn stats(&self) -> CacheStats {
        self.inner.lock().stats
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn bytes(&self) -> usize {
        self.inner.lock().bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::AudioFrame;
    use crate::time::TimeBase;

    fn frame(samples: usize, pts_ticks: i64) -> Arc<Frame> {
        Arc::new(Frame::Audio(AudioFrame {
            pts: Timestamp::new(pts_ticks, TimeBase::AUDIO_48K),
            sample_rate: 48_000,
            channels: vec![Arc::new(vec![0.5; samples]), Arc::new(vec![0.5; samples])],
        }))
    }

    fn key(hash: &str, ticks: i64) -> FrameKey {
        FrameKey {
            media_hash: hash.into(),
            stream: 1,
            ticks,
        }
    }

    #[test]
    fn hits_and_misses_are_counted() {
        let c = FrameCache::new(1 << 20);
        let k = key("aaa", 0);
        assert!(c.get(&k).is_none());
        c.insert(k.clone(), frame(128, 0));
        assert!(c.get(&k).is_some());
        let s = c.stats();
        assert_eq!((s.hits, s.misses), (1, 1));
        assert!((s.hit_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn evicts_least_recently_used_to_stay_within_the_byte_budget() {
        // Each frame is 2 channels * 1024 samples * 4 bytes = 8192 bytes.
        let c = FrameCache::new(8192 * 3);
        for i in 0..3 {
            c.insert(key("aaa", i), frame(1024, i));
        }
        assert_eq!(c.len(), 3);
        assert!(c.bytes() <= c.capacity_bytes());

        // Touch the oldest so it is no longer least-recently-used.
        assert!(c.get(&key("aaa", 0)).is_some());
        // Inserting a fourth must evict key 1, not key 0.
        c.insert(key("aaa", 3), frame(1024, 3));
        assert!(
            c.get(&key("aaa", 0)).is_some(),
            "recently used entry survived"
        );
        assert!(
            c.get(&key("aaa", 1)).is_none(),
            "least recently used was evicted"
        );
        assert!(c.stats().evictions >= 1);
        assert!(c.bytes() <= c.capacity_bytes(), "never exceeds the budget");
    }

    #[test]
    fn replacing_media_invalidates_only_its_own_frames() {
        let c = FrameCache::new(1 << 20);
        for i in 0..4 {
            c.insert(key("old", i), frame(64, i));
            c.insert(key("new", i), frame(64, i));
        }
        assert_eq!(c.len(), 8);
        let freed = c.invalidate_media("old");
        assert_eq!(freed, 4);
        assert_eq!(c.len(), 4);
        assert!(
            c.get(&key("old", 0)).is_none(),
            "stale media must not be served"
        );
        assert!(
            c.get(&key("new", 0)).is_some(),
            "unrelated media is untouched"
        );
    }

    #[test]
    fn a_frame_larger_than_the_budget_is_simply_not_cached() {
        let c = FrameCache::new(1024);
        c.insert(key("aaa", 0), frame(4096, 0)); // 32 KiB
        assert!(
            c.is_empty(),
            "must not evict everything for something that cannot fit"
        );
        assert_eq!(c.bytes(), 0);
    }
}
