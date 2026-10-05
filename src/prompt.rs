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

/// An answer `check` accepts, as it returns it.
pub fn text_as(
    question: &str,
    default: &str,
    check: fn(&str) -> Result<String, String>,
) -> Result<String> {
    let answer: String = cliclack::input(question)
        .default_input(default)
        .validate(move |input: &String| check(input).map(|_| ()))
        .interact()?;
    check(&answer).map_err(anyhow::Error::msg)
}

/// A free answer, with `placeholder` as an example rather than a default.
pub fn ask(question: &str, placeholder: &str) -> Result<String> {
    Ok(cliclack::input(question)
        .placeholder(placeholder)
        .interact()?)
}

/// A free answer, empty when the user just presses enter.
pub fn line(question: &str) -> Result<String> {
    Ok(cliclack::input(question).required(false).interact()?)
}

pub fn confirm(question: &str, default: bool) -> Result<bool> {
    Ok(cliclack::confirm(question)
        .initial_value(default)
        .interact()?)
}

pub fn intro(title: &str) -> Result<()> {
    Ok(cliclack::intro(title)?)
}

pub fn outro(message: &str) -> Result<()> {
    Ok(cliclack::outro(message)?)
}
