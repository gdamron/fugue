use super::*;

pub(super) fn normalize_sequence_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        index.min(len - 1)
    }
}

pub(super) fn parse_sequence_bank(
    value: Option<&Value>,
) -> Result<Vec<Vec<Step>>, Box<dyn std::error::Error>> {
    let Some(array) = value.and_then(|value| value.as_array()) else {
        return Ok(Vec::new());
    };

    if array.len() > MAX_SEQUENCES {
        return Err(format!(
            "sequence bank may not contain more than {} sequences",
            MAX_SEQUENCES
        )
        .into());
    }

    let mut bank = Vec::with_capacity(array.len());
    for sequence in array {
        let parsed = parse_pattern(Some(sequence))?;
        if parsed.len() > MAX_STEPS {
            return Err(format!(
                "each sequence may not contain more than {} steps",
                MAX_STEPS
            )
            .into());
        }
        bank.push(parsed);
    }

    Ok(bank)
}

pub(crate) fn parse_sequence_bank_json(value: &str) -> Result<Vec<Vec<Step>>, String> {
    let value: Value = serde_json::from_str(value).map_err(|err| err.to_string())?;
    parse_sequence_bank(Some(&value)).map_err(|err| err.to_string())
}
