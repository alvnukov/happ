// Go source marker: src/text/template/*
//
// This module mirrors Go package boundaries to simplify 1:1 logic transfers.

// Go source marker: src/text/template/parse/*
pub mod parse {
    pub use crate::go_compat::actionparse;
    pub use crate::go_compat::ident;
    pub use crate::go_compat::parse;
    pub use crate::go_compat::pipeline_decl;
    pub use crate::go_compat::tokenize;
    pub use crate::go_compat::trim;
    pub use crate::go_compat::varcheck;
}

// Go source marker: src/text/template/template.go + option-like behavior
pub mod template {
    pub use crate::go_compat::template;
}

// Go source marker: src/text/template/exec.go
pub mod exec {
    pub use crate::go_compat::call;
    pub use crate::go_compat::commandkind;
    pub use crate::go_compat::evaldiag;
    pub use crate::go_compat::externalfn;
    pub use crate::go_compat::path;
    pub use crate::go_compat::rangeeval;
    pub use crate::go_compat::truth;
    pub use crate::go_compat::typeutil;
    pub use crate::go_compat::valuefmt;
}

// Go source marker: src/text/template/funcs.go
pub mod funcs {
    pub use crate::go_compat::collections;
    pub use crate::go_compat::compare;
    pub use crate::go_compat::textfmt;
}
