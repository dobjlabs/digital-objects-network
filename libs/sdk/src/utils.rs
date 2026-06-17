//! Various utilities

use pod2::middleware::{NativeOperation, NativePredicate};

/// Return the operation that generates the predicate from entries.
/// Panics for predicates that have no corresponding operation.
pub(crate) fn native_pred_to_op(pred: NativePredicate) -> NativeOperation {
    match pred {
        NativePredicate::Equal => NativeOperation::EqualFromEntries,
        NativePredicate::NotEqual => NativeOperation::NotEqualFromEntries,
        NativePredicate::Lt => NativeOperation::LtFromEntries,
        NativePredicate::LtEq => NativeOperation::LtEqFromEntries,
        NativePredicate::Gt => NativeOperation::GtFromEntries,
        NativePredicate::GtEq => NativeOperation::GtEqFromEntries,
        NativePredicate::Contains => NativeOperation::ContainsFromEntries,
        NativePredicate::NotContains => NativeOperation::NotContainsFromEntries,
        NativePredicate::Sum => NativeOperation::SumFromEntries,
        NativePredicate::Product => NativeOperation::ProductFromEntries,
        NativePredicate::Max => NativeOperation::MaxFromEntries,
        NativePredicate::Hash => NativeOperation::HashFromEntries,
        NativePredicate::PublicKey => NativeOperation::PublicKeyFromEntries,
        NativePredicate::SignedBy => NativeOperation::SignedByFromEntries,
        NativePredicate::ContainerInsert => NativeOperation::ContainerInsertFromEntries,
        NativePredicate::ContainerUpdate => NativeOperation::ContainerUpdateFromEntries,
        NativePredicate::ContainerDelete => NativeOperation::ContainerDeleteFromEntries,
        NativePredicate::DictContains => NativeOperation::DictContainsFromEntries,
        NativePredicate::DictNotContains => NativeOperation::DictNotContainsFromEntries,
        NativePredicate::ArrayContains => NativeOperation::ArrayContainsFromEntries,
        NativePredicate::SetContains => NativeOperation::SetContainsFromEntries,
        NativePredicate::SetNotContains => NativeOperation::SetNotContainsFromEntries,
        NativePredicate::DictInsert => NativeOperation::DictInsertFromEntries,
        NativePredicate::DictUpdate => NativeOperation::DictUpdateFromEntries,
        NativePredicate::DictDelete => NativeOperation::DictDeleteFromEntries,
        NativePredicate::SetInsert => NativeOperation::SetInsertFromEntries,
        NativePredicate::SetDelete => NativeOperation::SetDeleteFromEntries,
        NativePredicate::ArrayUpdate => NativeOperation::ArrayUpdateFromEntries,
        _ => panic!("unused"),
    }
}
