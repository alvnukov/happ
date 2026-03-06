mod literals;
mod printf;

pub use literals::{
    decode_go_string_literal, parse_char_constant, parse_go_quoted_prefix, parse_number_value,
};
pub use printf::{
    format_float_exp_go, format_float_general_go, format_signed_integer_radix, go_printf,
    looks_like_char_literal, looks_like_numeric_literal, parse_width_zero_precision,
};

#[cfg(test)]
mod tests;
