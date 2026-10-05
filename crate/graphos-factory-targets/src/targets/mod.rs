//! The targets this tree carries. Each product binary (`src/bin/*.rs`) is
//! its own composition root and registers exactly one of them; the core
//! reads a target only through `graphos_factory_core::target::Target` and
//! never names one.

pub mod graphos;
