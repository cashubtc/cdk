//! Typestate markers for SendBatch state transitions

/// Marker for a batch with a built (unsigned) PSBT
#[derive(Debug, Clone)]
pub struct Built;

/// Marker for a batch with a signed transaction
#[derive(Debug, Clone)]
pub struct Signed;
