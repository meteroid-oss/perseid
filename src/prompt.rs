use anyhow::Result;

/// Checkboxes: space toggles, enter confirms, at least one required.
pub fn pick_many(question: &str, items: &[(&str, &str)]) -> Result<Vec<usize>> {
    let mut prompt = cliclack::multiselect(question).required(true);
    for (i, (item, hint)) in items.iter().enumerate() {
        prompt = prompt.item(i, item, hint);
    }
    Ok(prompt.interact()?)
}

pub fn pick_one(question: &str, items: &[String], default: usize) -> Result<usize> {
    let mut prompt = cliclack::select(question).initial_value(default);
    for (i, item) in items.iter().enumerate() {
        prompt = prompt.item(i, item, "");
    }
    Ok(prompt.interact()?)
}

pub fn text(question: &str, default: &str) -> Result<String> {
    Ok(cliclack::input(question)
        .default_input(default)
        .interact()?)
}

pub fn confirm(question: &str, default: bool) -> Result<bool> {
    Ok(cliclack::confirm(question)
        .initial_value(default)
        .interact()?)
}

pub fn intro(title: &str) -> Result<()> {
    Ok(cliclack::intro(title)?)
}
