// `explicit::explicit` mirrors the layout of the sibling `checkers/`,
// `generators/` and `creators/` directories, where each processor lives in a
// file named after itself. There is exactly one explicit processor, so the
// directory and the file share a name; renaming either would make this one
// module the odd one out.
#[allow(clippy::module_inception)]
mod explicit;
