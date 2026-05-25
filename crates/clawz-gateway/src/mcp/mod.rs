//! MCP (Model Context Protocol) server module for the ClawZ Gateway.
//!
//! This module exposes the gateway's capabilities — agents, tools, resources,
//! and metrics — via the MCP wire protocol. It acts as the bridge between
//! external MCP clients and the internal gateway services.
//!
//! The entry point is [`handle_mcp_request`], which parses JSON-RPC 2.0
//! requests and dispatches them to the appropriate built-in tool or resource
//! handler.
//!
//! # Re-exports
//!
//! - [`JsonRpcRequest`] / [`JsonRpcResponse`] — low-level JSON-RPC wire types.
//! - [`handle_mcp_request`] — Axum handler for incoming MCP requests.

pub mod server;

pub use server::{handle_mcp_request, JsonRpcRequest, JsonRpcResponse};
