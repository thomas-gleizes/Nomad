use thiserror::Error;

/// Erreurs du protocole / codec partagé.
#[derive(Debug, Error)]
pub enum Error {
    #[error("erreur de (dé)sérialisation: {0}")]
    Codec(#[from] bincode::Error),

    #[error("trame trop grande: {0} octets (max {max})", max = crate::codec::MAX_FRAME_LEN)]
    FrameTooLarge(usize),

    #[error("trame incomplète")]
    Incomplete,
}

pub type Result<T> = std::result::Result<T, Error>;
