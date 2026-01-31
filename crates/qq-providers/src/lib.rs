//! qq-providers: LLM provider implementations for quick-query
//!
//! This crate provides implementations of the Provider trait for various LLM APIs.

pub mod openai;

pub use openai::OpenAIProvider;
