//! roblox-ts's macro tables (`macros/*.ts`): calls, property calls, constructors and identifiers that compile to
//! inline Luau instead of a call. The macro manager resolves each entry's symbol.

pub mod call_macros;
pub mod constructor_macros;
pub mod identifier_macros;
pub mod property_call_macros;
pub mod types;

pub use call_macros::CALL_MACROS;
pub use constructor_macros::CONSTRUCTOR_MACROS;
pub use identifier_macros::IDENTIFIER_MACROS;
pub use property_call_macros::PROPERTY_CALL_MACROS;
