//! `jg agent-guide` — print the guide that teaches AI agents to use jg safely.
//! The text is compiled into the binary from docs/AGENT_GUIDE.md, so any agent in
//! any repository can run `jg agent-guide` and get instructions matching this version.

pub const GUIDE: &str = include_str!("../../docs/AGENT_GUIDE.md");
