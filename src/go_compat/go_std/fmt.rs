// Go source marker: src/fmt/*
//
// Canonical printf-compatible surface used by templates.
pub use crate::go_compat::compat::{
    format_float_exp_go, format_float_general_go, format_signed_integer_radix, go_printf,
    looks_like_char_literal, looks_like_numeric_literal, parse_width_zero_precision,
};
