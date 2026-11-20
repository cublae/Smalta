//! The launcher's calculator: `+ - * / % ^`, parentheses and decimals.

pub fn looks_like_math(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && text.chars().any(|c| c.is_ascii_digit())
        && text.chars().any(|c| "+-*/%^(".contains(c))
        && text.chars().all(|c| c.is_ascii_digit() || " .,+-*/%^()".contains(c))
}

pub fn eval(text: &str) -> Option<f64> {
    let tokens: Vec<char> =
        text.chars().filter(|c| !c.is_whitespace()).map(|c| if c == ',' { '.' } else { c }).collect();
    let mut parser = Parser { tokens, at: 0 };
    let value = parser.sum()?;
    (parser.at == parser.tokens.len() && value.is_finite()).then_some(value)
}

pub fn format(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        let text = format!("{value:.10}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

struct Parser {
    tokens: Vec<char>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.tokens.get(self.at).copied()
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.at += 1;
            let rhs = self.product()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        while let Some(op @ ('*' | '/' | '%')) = self.peek() {
            self.at += 1;
            let rhs = self.power()?;
            value = match op {
                '*' => value * rhs,
                '/' => value / rhs,
                _ => value % rhs,
            };
        }
        Some(value)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some('^') {
            self.at += 1;
            // Right-associative: 2^3^2 is 2^9.
            return Some(base.powf(self.power()?));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' => {
                self.at += 1;
                Some(-self.unary()?)
            }
            '+' => {
                self.at += 1;
                self.unary()
            }
            '(' => {
                self.at += 1;
                let value = self.sum()?;
                (self.peek() == Some(')')).then(|| self.at += 1)?;
                Some(value)
            }
            _ => {
                let start = self.at;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.at += 1;
                }
                self.tokens[start..self.at].iter().collect::<String>().parse().ok()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        assert_eq!(eval("2+2*3"), Some(8.0));
        assert_eq!(eval("(2+2)*3"), Some(12.0));
        assert_eq!(eval("2^3^2"), Some(512.0));
        assert_eq!(eval("-3 + 1,5"), Some(-1.5));
        assert_eq!(eval("1/0"), None);
        assert_eq!(eval("2+"), None);
        assert_eq!(format(0.1 + 0.2), "0.3");
    }
}
