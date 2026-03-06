pub mod cache;
pub mod fetcher;
pub mod instrumental;
pub mod kugou;
pub mod lrc_parser;
pub mod lrclib;
pub mod netease;
pub mod petitlyrics;
pub mod qqmusic;
pub mod types;

pub use fetcher::LyricsFetcher;
pub use types::LyricsInfo;
