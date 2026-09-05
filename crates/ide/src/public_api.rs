use hir::{
    Adt, AssocItem, AssocItemContainer, DisplayTarget, HasCrate, HasVisibility, HirDisplay,
    ModuleDef, Semantics, Visibility,
};
use ide_db::{FxHashMap, RootDatabase, SymbolKind};
use itertools::Itertools;
use syntax::{TextRange, TextSize};

use crate::{NavigationTarget, TryToNav, navigation_target::ToNav};

#[derive(Debug, Clone)]
pub struct PublicApi {
    pub text: String,
    pub module_name: String,
    pub mappings: Vec<PublicApiMapping>,
}

#[derive(Debug, Clone)]
pub struct PublicApiMapping {
    pub range: TextRange,
    pub target: NavigationTarget,
    pub name: String,
    pub kind: Option<SymbolKind>,
}

pub(crate) fn public_api(db: &RootDatabase, file_id: ide_db::FileId) -> PublicApi {
    let sema = Semantics::new(db);
    let Some(module) = sema.file_to_module_def(file_id) else {
        return PublicApi {
            text: "// No Rust module found for this file.\n".to_owned(),
            module_name: String::new(),
            mappings: Vec::new(),
        };
    };

    let display_target = module.krate(db).to_display_target(db);
    let edition = module.krate(db).edition(db);
    let module_name = module_path(db, module);
    let mut builder = PublicApiBuilder::default();

    let module_line = format!("pub mod {module_name}");
    let module_nav = module.to_nav(db).call_site;
    builder.push_mapped_line(module_line, module_nav, module_name.clone());

    let mut declarations = module.public_scope(db);
    declarations.sort_by_key(|(name, _)| name.display(db, edition).to_string());
    let public_paths = PublicApiPaths::new(db, &module_name, &declarations);

    for (name, def) in declarations {
        let path = match def {
            ModuleDef::Module(module) => module_path(db, module),
            _ => format!("{}::{}", module_name, name.display(db, edition)),
        };
        if let Some(line) = render_module_def(db, def, display_target, &path) {
            if let Some(nav) = def.try_to_nav(&sema).map(|it| it.call_site) {
                let name = nav.name.to_string();
                builder.push_mapped_line(line, nav, name);
            } else {
                builder.push_line(line);
            }
        }

        if let ModuleDef::Trait(trait_) = def {
            for item in trait_.items(db) {
                if let Some(line) = render_assoc_item(db, item, display_target, &public_paths) {
                    if let Some(nav) = item.try_to_nav(&sema).map(|it| it.call_site) {
                        let name = nav.name.to_string();
                        builder.push_mapped_line(line, nav, name);
                    } else {
                        builder.push_line(line);
                    }
                }
            }
        }
    }

    let mut impls = module.impl_defs(db);
    impls.sort_by_key(|impl_| render_impl_header(db, *impl_, display_target, &public_paths));

    for impl_ in impls {
        if !impl_is_public_api(db, impl_) {
            continue;
        }

        let is_trait_impl = impl_.trait_(db).is_some();
        let mut items = impl_.items(db);
        if !is_trait_impl {
            items.retain(|item| item.visibility(db) == Visibility::Public);
        }
        if items.is_empty() && !is_trait_impl {
            continue;
        }

        let Some(header) = render_impl_header(db, impl_, display_target, &public_paths) else {
            continue;
        };
        if let Some(nav) = impl_.try_to_nav(&sema).map(|it| it.call_site) {
            builder.push_mapped_line(header, nav, "impl".to_owned());
        } else {
            builder.push_line(header);
        }

        items.sort_by_key(|item| item.name(db).map(|name| name.display(db, edition).to_string()));
        for item in items {
            if let Some(line) = render_assoc_item(db, item, display_target, &public_paths) {
                if let Some(nav) = item.try_to_nav(&sema).map(|it| it.call_site) {
                    let name = nav.name.to_string();
                    builder.push_mapped_line(line, nav, name);
                } else {
                    builder.push_line(line);
                }
            }
        }
    }

    PublicApi { text: builder.text, module_name, mappings: builder.mappings }
}

struct PublicApiPaths {
    adts: FxHashMap<Adt, String>,
    traits: FxHashMap<hir::Trait, String>,
}

impl PublicApiPaths {
    fn new(db: &RootDatabase, module_name: &str, declarations: &[(hir::Name, ModuleDef)]) -> Self {
        let mut adts = FxHashMap::default();
        let mut traits = FxHashMap::default();
        for (name, def) in declarations {
            let path = format!("{}::{}", module_name, name.display(db, def.krate(db).edition(db)));
            match def {
                ModuleDef::Adt(adt) => {
                    adts.entry(*adt).or_insert(path);
                }
                ModuleDef::Trait(trait_) => {
                    traits.entry(*trait_).or_insert(path);
                }
                _ => {}
            }
        }
        Self { adts, traits }
    }

    fn adt_path(&self, adt: Adt) -> Option<&str> {
        self.adts.get(&adt).map(String::as_str)
    }

    fn trait_path(&self, trait_: hir::Trait) -> Option<&str> {
        self.traits.get(&trait_).map(String::as_str)
    }
}

#[derive(Default)]
struct PublicApiBuilder {
    text: String,
    mappings: Vec<PublicApiMapping>,
}

impl PublicApiBuilder {
    fn push_line(&mut self, line: String) {
        self.text.push_str(&line);
        self.text.push('\n');
    }

    fn push_mapped_line(&mut self, line: String, target: NavigationTarget, name: String) {
        let start = TextSize::of(&self.text);
        let len = TextSize::of(&line);
        self.text.push_str(&line);
        self.text.push('\n');
        self.mappings.push(PublicApiMapping {
            range: TextRange::at(start, len),
            kind: target.kind,
            target,
            name,
        });
    }
}

fn render_module_def(
    db: &RootDatabase,
    def: ModuleDef,
    display_target: DisplayTarget,
    path: &str,
) -> Option<String> {
    let line = match def {
        ModuleDef::Module(module) => format!("pub mod {}", module_path(db, module)),
        ModuleDef::Function(function) => qualify_signature(
            function.display(db, display_target).to_string(),
            "fn",
            &function.name(db).display(db, function.krate(db).edition(db)).to_string(),
            path,
        ),
        ModuleDef::Adt(adt) => render_adt(db, adt, display_target, path)?,
        ModuleDef::Const(konst) => qualify_signature(
            konst.display(db, display_target).to_string(),
            "const",
            &konst.name(db)?.display(db, konst.krate(db).edition(db)).to_string(),
            path,
        ),
        ModuleDef::Static(statik) => qualify_signature(
            statik.display(db, display_target).to_string(),
            "static",
            &statik.name(db).display(db, statik.krate(db).edition(db)).to_string(),
            path,
        ),
        ModuleDef::Trait(trait_) => qualify_signature(
            trait_.display(db, display_target).to_string(),
            "trait",
            &trait_.name(db).display(db, trait_.krate(db).edition(db)).to_string(),
            path,
        ),
        ModuleDef::TypeAlias(type_alias) => qualify_signature(
            type_alias.display(db, display_target).to_string(),
            "type",
            &type_alias.name(db).display(db, type_alias.krate(db).edition(db)).to_string(),
            path,
        ),
        ModuleDef::Macro(macro_) => qualify_signature(
            macro_.display(db, display_target).to_string(),
            "macro_rules!",
            &macro_.name(db).display(db, macro_.krate(db).edition(db)).to_string(),
            path,
        ),
        ModuleDef::EnumVariant(_) | ModuleDef::BuiltinType(_) => return None,
    };
    Some(single_line(line))
}

fn render_adt(
    db: &RootDatabase,
    adt: Adt,
    display_target: DisplayTarget,
    path: &str,
) -> Option<String> {
    let (keyword, name, signature) = match adt {
        Adt::Struct(it) => (
            "struct",
            it.name(db).display(db, it.krate(db).edition(db)).to_string(),
            it.display_limited(db, Some(usize::MAX), display_target)
                .with_private_fields(false)
                .to_string(),
        ),
        Adt::Enum(it) => (
            "enum",
            it.name(db).display(db, it.krate(db).edition(db)).to_string(),
            it.display_limited(db, Some(usize::MAX), display_target).to_string(),
        ),
        Adt::Union(it) => (
            "union",
            it.name(db).display(db, it.krate(db).edition(db)).to_string(),
            it.display_limited(db, Some(usize::MAX), display_target)
                .with_private_fields(false)
                .to_string(),
        ),
    };
    Some(single_line(qualify_signature(signature, keyword, &name, path)))
}

fn render_impl_header(
    db: &RootDatabase,
    impl_: hir::Impl,
    display_target: DisplayTarget,
    public_paths: &PublicApiPaths,
) -> Option<String> {
    let self_ty = rendered_impl_self_ty(db, impl_, display_target, public_paths);
    let line = if let Some(trait_) = impl_.trait_(db) {
        let trait_path = public_paths
            .trait_path(trait_)
            .map(str::to_owned)
            .or_else(|| def_path(db, ModuleDef::Trait(trait_)))
            .unwrap_or_else(|| {
                trait_
                    .display(db, display_target)
                    .to_string()
                    .trim_start_matches("trait ")
                    .to_owned()
            });
        format!("impl {trait_path} for {self_ty}")
    } else {
        format!("impl {self_ty}")
    };
    Some(single_line(line))
}

fn render_assoc_item(
    db: &RootDatabase,
    item: AssocItem,
    display_target: DisplayTarget,
    public_paths: &PublicApiPaths,
) -> Option<String> {
    let name = item.name(db)?.display(db, item.module(db).krate(db).edition(db)).to_string();
    let container = item.container(db);
    let is_trait_impl =
        matches!(container, AssocItemContainer::Impl(impl_) if impl_.trait_(db).is_some());
    let owner = match container {
        AssocItemContainer::Trait(trait_) => public_paths
            .trait_path(trait_)
            .map(str::to_owned)
            .or_else(|| def_path(db, ModuleDef::Trait(trait_)))?,
        AssocItemContainer::Impl(impl_) => {
            rendered_impl_self_ty(db, impl_, display_target, public_paths)
        }
    };
    let path = format!("{owner}::{name}");
    let line = match item {
        AssocItem::Function(function) => {
            qualify_signature(function.display(db, display_target).to_string(), "fn", &name, &path)
        }
        AssocItem::Const(konst) => {
            qualify_signature(konst.display(db, display_target).to_string(), "const", &name, &path)
        }
        AssocItem::TypeAlias(type_alias) => qualify_signature(
            type_alias.display(db, display_target).to_string(),
            "type",
            &name,
            &path,
        ),
    };
    let line = if is_trait_impl { ensure_pub(line) } else { line };
    Some(single_line(line))
}

fn ensure_pub(line: String) -> String {
    if line.starts_with("pub ") { line } else { format!("pub {line}") }
}

fn impl_is_public_api(db: &RootDatabase, impl_: hir::Impl) -> bool {
    let self_ty_is_public =
        impl_.self_ty(db).as_adt().is_none_or(|adt| adt.visibility(db) == Visibility::Public);
    let trait_is_public =
        impl_.trait_(db).is_none_or(|trait_| trait_.visibility(db) == Visibility::Public);
    self_ty_is_public && trait_is_public
}

fn rendered_impl_self_ty(
    db: &RootDatabase,
    impl_: hir::Impl,
    display_target: DisplayTarget,
    public_paths: &PublicApiPaths,
) -> String {
    let self_ty = impl_.self_ty(db);
    self_ty
        .as_adt()
        .and_then(|adt| {
            public_paths
                .adt_path(adt)
                .map(str::to_owned)
                .or_else(|| def_path(db, ModuleDef::Adt(adt)))
        })
        .unwrap_or_else(|| self_ty.display(db, display_target).to_string())
}

fn def_path(db: &RootDatabase, def: ModuleDef) -> Option<String> {
    match def {
        ModuleDef::Module(module) => Some(module_path(db, module)),
        _ => {
            let edition = def.krate(db).edition(db);
            let module = def.module(db)?;
            let name = def.name(db)?.display(db, edition).to_string();
            Some(format!("{}::{name}", module_path(db, module)))
        }
    }
}

fn module_path(db: &RootDatabase, module: hir::Module) -> String {
    let edition = module.krate(db).edition(db);
    let krate = module
        .krate(db)
        .display_name(db)
        .map(|name| name.to_string())
        .unwrap_or_else(|| "crate".to_owned());
    std::iter::once(krate)
        .chain(module.path_segments(db).map(|name| name.display(db, edition).to_string()))
        .join("::")
}

fn qualify_signature(mut signature: String, keyword: &str, name: &str, path: &str) -> String {
    let needle = format!("{keyword} {name}");
    if let Some(start) = signature.find(&needle) {
        let name_start = start + keyword.len() + 1;
        let name_end = name_start + name.len();
        signature.replace_range(name_start..name_end, path);
    }
    signature
}

fn single_line(line: String) -> String {
    line.lines().join(" ")
}

#[cfg(test)]
mod tests {
    use crate::fixture;

    #[test]
    fn renders_public_api_for_file_module() {
        let (analysis, position) = fixture::position(
            r#"
//- /lib.rs crate:rocks
pub mod arith;

//- /arith.rs
pub mod int;

//- /arith/int.rs
$0
pub struct Int;

impl Int {
    pub fn zero() -> Self { Self }
    fn private() -> Self { Self }
}

pub fn helper() {}
fn private_free() {}

struct Hidden;
impl Hidden {
    pub fn leaked_if_not_filtered() {}
}
"#,
        );

        let public_api = analysis.public_api(position.file_id).unwrap();

        assert!(public_api.text.contains("pub mod rocks::arith::int"), "{}", public_api.text);
        assert!(
            public_api.text.contains("pub struct rocks::arith::int::Int"),
            "{}",
            public_api.text
        );
        assert!(
            public_api.text.contains("pub fn rocks::arith::int::helper"),
            "{}",
            public_api.text
        );
        assert!(
            public_api.text.contains("pub fn rocks::arith::int::Int::zero"),
            "{}",
            public_api.text
        );
        assert!(!public_api.text.contains("private"), "{}", public_api.text);
        assert!(!public_api.text.contains("leaked_if_not_filtered"), "{}", public_api.text);
        assert!(!public_api.mappings.is_empty());
    }

    #[test]
    fn renders_public_reexports_and_item_children() {
        let (analysis, position) = fixture::position(
            r#"
//- /lib.rs crate:rocks
pub mod arith;

//- /arith.rs
$0
mod int;
pub use int::Int;

impl Int {
    pub fn zero() -> Self { Self }
}

pub trait ToText {
    fn to_text(&self) -> String;
}

impl ToText for Int {
    fn to_text(&self) -> String { String::new() }
}

pub trait Marker {}
impl Marker for Int {}

pub trait Solver {
    type Model;
    const COMPLETE: bool;
    fn solve(&self) -> Self::Model;
}

pub enum ResultKind {
    Sat,
    Unsat,
}

pub struct Config {
    pub limit: usize,
    secret: usize,
}

//- /arith/int.rs
pub struct Int;
"#,
        );

        let public_api = analysis.public_api(position.file_id).unwrap();

        assert!(public_api.text.contains("pub struct rocks::arith::Int"), "{}", public_api.text);
        assert!(
            public_api.text.contains("impl rocks::arith::Marker for rocks::arith::Int"),
            "{}",
            public_api.text
        );
        assert!(
            public_api.text.contains("impl rocks::arith::ToText for rocks::arith::Int"),
            "{}",
            public_api.text
        );
        assert!(public_api.text.contains("pub fn rocks::arith::Int::zero"), "{}", public_api.text);
        assert!(
            public_api.text.contains("pub fn rocks::arith::Int::to_text"),
            "{}",
            public_api.text
        );
        assert!(public_api.text.contains("pub trait rocks::arith::Solver"), "{}", public_api.text);
        assert!(
            public_api.text.contains("pub type rocks::arith::Solver::Model"),
            "{}",
            public_api.text
        );
        assert!(
            public_api.text.contains("pub const rocks::arith::Solver::COMPLETE"),
            "{}",
            public_api.text
        );
        assert!(
            public_api.text.contains("pub fn rocks::arith::Solver::solve"),
            "{}",
            public_api.text
        );
        assert!(public_api.text.contains("Sat"), "{}", public_api.text);
        assert!(public_api.text.contains("pub limit: usize"), "{}", public_api.text);
        assert!(!public_api.text.contains("secret"), "{}", public_api.text);
    }
}
