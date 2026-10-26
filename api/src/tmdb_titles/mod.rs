pub mod export;
pub mod localize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleMedia {
    Movie,
    Tv,
}

impl TitleMedia {
    pub const ALL: [Self; 2] = [Self::Movie, Self::Tv];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Tv => "tv",
        }
    }
}
