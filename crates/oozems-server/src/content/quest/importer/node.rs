use super::*;

pub(crate) fn read_u32_list(
    quest_id: u32,
    node: &WzNodeArc,
    name: &str,
) -> Result<Vec<u32>, QuestContentError> {
    let Some(values) = wz::child(node, name)? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for value in wz::sorted_children(&values)? {
        output.push(
            u32::try_from(scalar_i64(&value)?.ok_or_else(|| {
                invalid(quest_id, format!("{name} contains a non-integer value"))
            })?)
            .map_err(|_| invalid(quest_id, format!("{name} contains a negative value")))?,
        );
    }
    output.sort_unstable();
    output.dedup();
    Ok(output)
}

pub(crate) fn validate_children(
    quest_id: u32,
    node: &WzNodeArc,
    allowed: &[&str],
    context: &str,
) -> Result<(), QuestContentError> {
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        if !allowed.contains(&name.as_str()) {
            return Err(unsupported(
                quest_id,
                format!("{context} metadata"),
                format!("{context} field {name:?} is not supported"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_exact_children(
    quest_id: u32,
    node: &WzNodeArc,
    expected: &[&str],
    context: &str,
) -> Result<(), QuestContentError> {
    let actual = wz::sorted_children(node)?
        .iter()
        .map(wz::node_name)
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = expected
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(invalid(
            quest_id,
            format!("{context} has fields {actual:?}, expected exactly {expected:?}"),
        ));
    }
    Ok(())
}

pub(crate) fn required_child(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<WzNodeArc, QuestContentError> {
    wz::child(node, name)?
        .ok_or_else(|| invalid(quest_id, format!("required node {name:?} is missing")))
}

pub(crate) fn required_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<u32, QuestContentError> {
    optional_u32(node, name, quest_id)?
        .ok_or_else(|| invalid(quest_id, format!("required integer {name:?} is missing")))
}

pub(crate) fn required_positive_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<u32, QuestContentError> {
    required_u32(node, name, quest_id).and_then(|value| {
        (value > 0)
            .then_some(value)
            .ok_or_else(|| invalid(quest_id, format!("integer {name:?} must be positive")))
    })
}

pub(crate) fn optional_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                invalid(
                    quest_id,
                    format!("integer {name:?} is negative or too large"),
                )
            })
        })
        .transpose()
}

pub(crate) fn optional_positive_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    optional_u32(node, name, quest_id)?
        .map(|value| {
            (value > 0)
                .then_some(value)
                .ok_or_else(|| invalid(quest_id, format!("integer {name:?} must be positive")))
        })
        .transpose()
}

pub(crate) fn optional_strict_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    if let Some(value) = scalar_i64(&value)? {
        return u32::try_from(value).map(Some).map_err(|_| {
            invalid(
                quest_id,
                format!("integer {name:?} is negative or too large"),
            )
        });
    }
    let source = raw_scalar_string(&value)?.ok_or_else(|| {
        invalid(
            quest_id,
            format!("property {name:?} is not an integer or strictly decimal string"),
        )
    })?;
    crate::quest_records::strict_decimal(&source)
        .and_then(|value| u32::try_from(value).ok())
        .map(Some)
        .ok_or_else(|| {
            invalid(
                quest_id,
                format!("property {name:?} is not a valid u32 decimal value"),
            )
        })
}

pub(crate) fn optional_positive_u64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u64>, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .map(|value| {
            u64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid(quest_id, format!("integer {name:?} must be positive")))
        })
        .transpose()
}

pub(crate) fn optional_nonnegative_u64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u64>, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| invalid(quest_id, format!("integer {name:?} must not be negative")))
        })
        .transpose()
}

pub(crate) fn required_i64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<i64, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .ok_or_else(|| invalid(quest_id, format!("required integer {name:?} is missing")))
}

pub(crate) fn optional_i64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<i64>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    scalar_i64(&value)?.map(Some).ok_or_else(|| {
        invalid(
            quest_id,
            format!("property {name:?} is not a supported integer"),
        )
    })
}

pub(crate) fn optional_bool(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<bool>, QuestContentError> {
    optional_i64(node, name, quest_id).map(|value| value.map(|value| value != 0))
}

pub(crate) fn required_nonempty_string(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<String, QuestContentError> {
    optional_nonempty_string(node, name, quest_id)?.ok_or_else(|| {
        invalid(
            quest_id,
            format!("required string {name:?} is missing or empty"),
        )
    })
}

pub(crate) fn optional_nonempty_string(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<String>, QuestContentError> {
    optional_string(node, name).and_then(|value| {
        value
            .map(|value| {
                let value = value.trim().to_owned();
                (!value.is_empty())
                    .then_some(value)
                    .ok_or_else(|| invalid(quest_id, format!("string {name:?} is empty")))
            })
            .transpose()
    })
}

pub(crate) fn optional_string(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<String>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    scalar_string(&value).map(|value| value.map(normalize_text))
}

pub(crate) fn scalar_i64(node: &WzNodeArc) -> Result<Option<i64>, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest integer value",
    })?;
    Ok(read
        .try_as_int()
        .map(|value| i64::from(*value))
        .or_else(|| read.try_as_short().map(|value| i64::from(*value)))
        .or_else(|| read.try_as_long().copied()))
}

pub(crate) fn scalar_string(node: &WzNodeArc) -> Result<Option<String>, QuestContentError> {
    raw_scalar_string(node).map(|value| value.map(normalize_text))
}

pub(crate) fn raw_scalar_string(node: &WzNodeArc) -> Result<Option<String>, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest string value",
    })?;
    Ok(read
        .try_as_string()
        .and_then(|value| value.get_string().ok()))
}

pub(crate) fn required_record_string(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<String, QuestContentError> {
    let value = raw_scalar_string(node)?
        .ok_or_else(|| invalid(quest_id, format!("{context} is not a string")))?;
    crate::quest_records::validate_value(&value)
        .map_err(|error| invalid(quest_id, format!("{context}: {error}")))?;
    Ok(value)
}

pub(crate) fn is_null(node: &WzNodeArc) -> Result<bool, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest null value",
    })?;
    Ok(read.is_null())
}

pub(crate) fn normalize_text(value: String) -> String {
    value.replace("\\n", "\n")
}
