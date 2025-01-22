use std::hash::Hash;

use oxc::{
  ast::ast::TSType,
  semantic::SymbolId,
  span::{Atom, SPAN},
};
use rustc_hash::FxHashSet;

use super::{property_key::PropertyKeyType, unresolved::UnresolvedType, Ty};
use crate::{analyzer::Analyzer, utils::F64WithEq};

#[derive(Debug, Default, Clone)]
pub enum UnionTypeBuilder<'a> {
  #[default]
  Never,
  Error,
  Any,
  Unknown,
  Compound(Box<UnionType<'a>>),
}

impl<'a> UnionTypeBuilder<'a> {
  pub fn add(&mut self, analyzer: &mut Analyzer<'a>, ty: Ty<'a>) {
    match (self, ty) {
      (UnionTypeBuilder::Error | UnionTypeBuilder::Any | UnionTypeBuilder::Unknown, _) => {}
      (s, Ty::Error | Ty::Generic(_) | Ty::Intrinsic(_) | Ty::Namespace(_)) => {
        *s = UnionTypeBuilder::Error
      }
      (s, Ty::Any) => *s = UnionTypeBuilder::Any,
      (s, Ty::Unknown) => *s = UnionTypeBuilder::Unknown,
      (_, Ty::Never) => {}

      (s, Ty::Union(tys)) => {
        tys.for_each(|ty| s.add(analyzer, ty));
      }

      (s, Ty::Instance(u)) => {
        let resolved = analyzer.unwrap_generic_instance(u);
        s.add(analyzer, resolved);
      }

      // The rest should be added to compound
      (s @ UnionTypeBuilder::Never, c) => {
        let mut compound = Box::new(UnionType::default());
        compound.add(c);
        *s = UnionTypeBuilder::Compound(compound);
      }
      (UnionTypeBuilder::Compound(compound), c) => {
        compound.add(c);
      }
    }
  }

  pub fn build(self, analyzer: &mut Analyzer<'a>) -> Ty<'a> {
    match self {
      UnionTypeBuilder::Never => Ty::Never,
      UnionTypeBuilder::Error => Ty::Error,
      UnionTypeBuilder::Any => Ty::Any,
      UnionTypeBuilder::Unknown => Ty::Unknown,
      UnionTypeBuilder::Compound(compound) => Ty::Union(analyzer.allocator.alloc(compound)),
    }
  }
}

#[derive(Debug, Default, Clone)]
pub struct UnionType<'a> {
  pub string: LiteralAble<&'a Atom<'a>>,
  pub number: LiteralAble<F64WithEq>,
  pub bigint: LiteralAble<&'a Atom<'a>>,
  pub symbol: LiteralAble<SymbolId>,

  pub object: bool,
  pub void: bool,
  pub null: bool,
  pub undefined: bool,
  /// (has_true, has_false)
  pub boolean: (bool, bool),

  pub complex: FxHashSet<Ty<'a>>,
  pub unresolved: Vec<UnresolvedType<'a>>,
}

impl<'a> UnionType<'a> {
  pub fn add(&mut self, ty: Ty<'a>) {
    match ty {
      Ty::Void => self.void = true,
      Ty::Null => self.null = true,
      Ty::Undefined => self.undefined = true,
      Ty::Object => self.object = true,

      Ty::String => self.string = LiteralAble::Any,
      Ty::Number => self.number = LiteralAble::Any,
      Ty::BigInt => self.bigint = LiteralAble::Any,
      Ty::Symbol => self.symbol = LiteralAble::Any,
      Ty::Boolean => self.boolean = (true, true),

      Ty::StringLiteral(s) => self.string.add(s),
      Ty::NumericLiteral(n) => self.number.add(n),
      Ty::BigIntLiteral(b) => self.bigint.add(b),
      Ty::UniqueSymbol(s) => self.symbol.add(s),
      Ty::BooleanLiteral(true) => self.boolean.0 = true,
      Ty::BooleanLiteral(false) => self.boolean.1 = true,

      Ty::Record(_)
      | Ty::Function(_)
      | Ty::Constructor(_)
      | Ty::Interface(_)
      | Ty::Namespace(_)
      | Ty::Intersection(_) => {
        self.complex.insert(ty);
      }

      Ty::Unresolved(unresolved) => self.unresolved.push(unresolved),

      _ => unreachable!("Handled in UnionTypeBuilder"),
    }
  }

  pub fn for_each(&self, mut f: impl FnMut(Ty<'a>) -> ()) {
    self.string.for_each(Ty::String, Ty::StringLiteral, &mut f);
    self.number.for_each(Ty::Number, Ty::NumericLiteral, &mut f);
    self.bigint.for_each(Ty::BigInt, Ty::BigIntLiteral, &mut f);
    self.symbol.for_each(Ty::Symbol, Ty::UniqueSymbol, &mut f);

    if self.object {
      f(Ty::Object);
    }
    if self.void {
      f(Ty::Void);
    }
    if self.null {
      f(Ty::Null);
    }
    if self.undefined {
      f(Ty::Undefined);
    }
    match (self.boolean.0, self.boolean.1) {
      (true, true) => f(Ty::Boolean),
      (true, false) => f(Ty::BooleanLiteral(true)),
      (false, true) => f(Ty::BooleanLiteral(false)),
      (false, false) => {}
    }

    self.complex.iter().copied().for_each(&mut f);
    self.unresolved.iter().copied().map(Ty::Unresolved).for_each(f);
  }
}

#[derive(Debug, Default, Clone)]
pub enum LiteralAble<L> {
  #[default]
  Vacant,
  Any,
  Literals(FxHashSet<L>),
}

impl<'a, L> LiteralAble<L> {
  pub fn add(&mut self, literal: L)
  where
    L: Hash + Eq,
  {
    match self {
      LiteralAble::Vacant => {
        *self = LiteralAble::Literals({
          let mut set = FxHashSet::default();
          set.insert(literal);
          set
        })
      }
      LiteralAble::Any => {}
      LiteralAble::Literals(set) => {
        set.insert(literal);
      }
    }
  }

  pub fn for_each(&self, any: Ty<'a>, ctor: fn(L) -> Ty<'a>, mut f: impl FnMut(Ty<'a>) -> ())
  where
    L: Copy,
  {
    match self {
      LiteralAble::Vacant => {}
      LiteralAble::Any => f(any),
      LiteralAble::Literals(set) => {
        set.iter().copied().map(ctor).for_each(&mut f as &mut dyn FnMut(Ty<'a>) -> ())
      }
    }
  }
}

impl<'a> Analyzer<'a> {
  pub fn into_union<Iter>(
    &mut self,
    types: impl IntoIterator<Item = Ty<'a>, IntoIter = Iter>,
  ) -> Ty<'a>
  where
    Iter: Iterator<Item = Ty<'a>> + ExactSizeIterator,
  {
    let mut iter = types.into_iter();
    match iter.len() {
      // FIXME: Should be Ty::Never
      0 => Ty::Undefined,
      1 => iter.next().unwrap(),
      _ => {
        let mut builder = UnionTypeBuilder::default();
        iter.for_each(|ty| builder.add(self, ty));
        builder.build(self)
      }
    }
  }

  pub fn get_optional_type(&mut self, optional: bool, ty: Ty<'a>) -> Ty<'a> {
    if optional {
      self.into_union([Ty::Undefined, ty])
    } else {
      ty
    }
  }

  pub fn get_union_property(&mut self, union: &UnionType<'a>, key: PropertyKeyType<'a>) -> Ty<'a> {
    let mut builder = UnionTypeBuilder::default();
    union.for_each(|ty| {
      let property = self.get_property(ty, key);
      builder.add(self, property)
    });
    builder.build(self)
  }

  pub fn print_union_type(&self, union: &UnionType<'a>) -> TSType<'a> {
    let mut types = self.ast_builder.vec();
    union.for_each(|ty| types.push(self.print_type(ty)));
    self.ast_builder.ts_type_union_type(SPAN, types)
  }
}
