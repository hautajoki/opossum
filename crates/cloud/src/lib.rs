//! The Open Cloud client: HTTP and Roblox's wire formats for assets, place versions, Luau execution and resources,
//! with its retry, timeout and error rules, plus the mock server tests run against. It knows nothing about project
//! files, the lock or config; bodies are bytes the caller supplies.
