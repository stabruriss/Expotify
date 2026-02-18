pub mod types;
pub mod lrc_parser;
pub mod cache;
pub mod instrumental;
pub mod netease;
pub mod qqmusic;
pub mod kugou;
pub mod lrclib;
pub mod petitlyrics;
pub mod fetcher;

pub use types::LyricsInfo;
pub use fetcher::LyricsFetcher;
