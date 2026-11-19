use poise::serenity_prelude as serenity;
use poise::serenity_prelude::Mentionable;
use rand::Rng;

const MAX_RANDOM_PICKS: usize = 3;
const MAX_EXPANSION_DEPTH: usize = 10;

fn parse_pick_options(input: &str) -> Vec<String> {
    let mut options = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut token_started = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            token_started = true;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            token_started = true;
            continue;
        }

        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            token_started = true;
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                token_started = true;
            }
            ch if ch.is_whitespace() => {
                if token_started {
                    options.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            _ => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if escaped {
        current.push('\\');
    }

    if token_started {
        options.push(current);
    }

    if quote.is_some() {
        return Vec::new();
    }

    options
}

fn random_pick(input: &str) -> String {
    let options = parse_pick_options(input);

    if options.is_empty() {
        return String::new();
    }

    let mut rng = rand::thread_rng();
    options[rng.gen_range(0..options.len())].clone()
}

fn random_range(input: &str) -> Option<String> {
    let input = input.trim();

    // Supports:
    // 1-10, 1 - 10, -5--1, -5-10.
    for (index, ch) in input.char_indices() {
        if ch != '-' || index == 0 {
            continue;
        }

        let left = input[..index].trim().parse::<i64>().ok()?;
        let right = input[index + 1..].trim().parse::<i64>().ok()?;

        let min = left.min(right);
        let max = left.max(right);

        let mut rng = rand::thread_rng();
        return Some(rng.gen_range(min..=max).to_string());
    }

    None
}

fn find_closing_brace(input: &str, start: usize) -> Option<usize> {
    let mut nested_depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for (offset, ch) in input[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            '{' => nested_depth += 1,
            '}' => {
                if nested_depth == 0 {
                    return Some(start + offset);
                }

                nested_depth -= 1;
            }
            _ => {}
        }
    }

    None
}

fn user_display_name(message: &serenity::Message) -> String {
    message
        .member
        .as_ref()
        .and_then(|member| member.nick.as_deref())
        .or(message.author.global_name.as_deref())
        .unwrap_or(&message.author.name)
        .to_string()
}

fn expand_inner(
    response: &str,
    message: &serenity::Message,
    count: Option<i64>,
    random_pick_count: &mut usize,
    depth: usize,
) -> String {
    // Prevent runaway recursive expansion.
    if depth >= MAX_EXPANSION_DEPTH {
        return response.to_string();
    }

    let mut output = String::new();
    let mut cursor = 0;

    while let Some(relative_start) = response[cursor..].find("${") {
        let start = cursor + relative_start;
        output.push_str(&response[cursor..start]);

        let expression_start = start + 2;

        let Some(end) = find_closing_brace(response, expression_start) else {
            output.push_str(&response[start..]);
            return output;
        };

        let expression = response[expression_start..end].trim();

        let replacement = if expression == "user" {
            message.author.mention().to_string()
        } else if expression == "user.silent" {
            user_display_name(message)
        } else if expression == "touser" {
            message
                .mentions
                .first()
                .map(|user| user.mention().to_string())
                .unwrap_or_else(|| "someone".to_string())
        } else if expression == "touser.silent" {
            message
                .mentions
                .first()
                .map(|user| {
                    user.global_name
                        .as_deref()
                        .unwrap_or(&user.name)
                        .to_string()
                })
                .unwrap_or_else(|| "someone".to_string())
        } else if expression == "channel" {
            message.channel_id.mention().to_string()
        } else if expression == "count" {
            count.unwrap_or_default().to_string()
        } else if let Some(arguments) = expression
            .strip_prefix("random.pick")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            if *random_pick_count < MAX_RANDOM_PICKS {
                *random_pick_count += 1;

                let selected = random_pick(arguments.trim_start());

                // Recursively expand variables inside the selected option.
                expand_inner(&selected, message, count, random_pick_count, depth + 1)
            } else {
                format!("${{{expression}}}")
            }
        } else if let Some(arguments) = expression.strip_prefix("random.") {
            random_range(arguments).unwrap_or_else(|| format!("${{{expression}}}"))
        } else {
            format!("${{{expression}}}")
        };

        output.push_str(&replacement);
        cursor = end + 1;
    }

    output.push_str(&response[cursor..]);
    output
}

pub fn expand(response: &str, message: &serenity::Message, count: Option<i64>) -> String {
    let mut random_pick_count = 0;

    expand_inner(response, message, count, &mut random_pick_count, 0)
}
