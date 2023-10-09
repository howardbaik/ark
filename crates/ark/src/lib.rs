//
// lib.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

pub mod browser;
pub mod control;
pub mod dap;
pub mod data_viewer;
pub mod environment;
pub mod errors;
pub mod frontend;
pub mod help_proxy;
pub mod interface;
pub mod kernel;
pub mod logger;
pub mod lsp;
pub mod modules;
pub mod plots;
pub mod r_task;
pub mod request;
pub mod shell;
pub mod version;

pub use r_task::r_task;

use serde::Deserialize;
use serde::Serialize;

#[macro_export]
macro_rules! r_safely {
    ($($expr:tt)*) => {{
        #[allow(unused_unsafe)]
        ark::r_task::safely(|| {
            unsafe { $($expr)* } }
        )
    }}
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, Default, Deserialize, Serialize)]
pub struct Position {
    row: usize,
    column: usize,
}

impl From<Position> for tree_sitter::Point {
    fn from(value: Position) -> Self {
        Self {
            row: value.row,
            column: value.column,
        }
    }
}

impl From<tree_sitter::Point> for Position {
    fn from(value: tree_sitter::Point) -> Self {
        Self {
            row: value.row,
            column: value.column,
        }
    }
}

impl From<Position> for tower_lsp::lsp_types::Position {
    fn from(value: Position) -> Self {
        Self {
            line: value.row as u32,
            character: value.column as u32,
        }
    }
}

impl From<tower_lsp::lsp_types::Position> for Position {
    fn from(value: tower_lsp::lsp_types::Position) -> Self {
        Self {
            row: value.line as usize,
            column: value.character as usize,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, Default, Deserialize, Serialize)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

impl From<tree_sitter::Range> for Range {
    fn from(value: tree_sitter::Range) -> Self {
        Self {
            start: value.start_point.into(),
            end: value.end_point.into(),
        }
    }
}

impl From<Range> for tower_lsp::lsp_types::Range {
    fn from(value: Range) -> Self {
        Self {
            start: value.start.into(),
            end: value.end.into(),
        }
    }
}

impl From<tower_lsp::lsp_types::Range> for Range {
    fn from(value: tower_lsp::lsp_types::Range) -> Self {
        Self {
            start: value.start.into(),
            end: value.end.into(),
        }
    }
}
