mod check;
mod completion;
mod highlight;
mod listscopes;
mod listthemes;
mod tokenize;

pub use check::{check, check_config, init_check_logger};
pub use completion::completion;
pub use highlight::highlight;
pub use listscopes::list_scopes;
pub use listthemes::list_themes;
pub use tokenize::tokenize;
