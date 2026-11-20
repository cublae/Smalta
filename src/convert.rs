//! The launcher's converter: `10 km in mi`, `72 f to c`, `100 usd в rub`.
//!
//! Units of one kind convert through a base unit each; temperatures, whose
//! scales do not start at the same zero, through Kelvin; currencies through
//! the dollar, at rates fetched on demand (see `services::rates`).

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Dimension {
    Length,
    Mass,
    Volume,
    Time,
    Data,
    Speed,
    Area,
}

/// Every name a unit goes by, its dimension, and its size in the base unit.
const UNITS: &[(&[&str], Dimension, f64)] = {
    use Dimension::*;
    &[
        (&["mm", "мм"], Length, 0.001),
        (&["cm", "см"], Length, 0.01),
        (&["m", "м", "meter", "meters", "metre", "metres"], Length, 1.0),
        (&["km", "км"], Length, 1000.0),
        (&["in", "inch", "inches", "\"", "дюйм", "дюймов", "дюйма"], Length, 0.0254),
        (&["ft", "foot", "feet", "'", "фут", "футов"], Length, 0.3048),
        (&["yd", "yard", "yards", "ярд"], Length, 0.9144),
        (&["mi", "mile", "miles", "миля", "мили", "миль"], Length, 1609.344),
        (&["nmi", "морская миля"], Length, 1852.0),
        (&["mg", "мг"], Mass, 1e-6),
        (&["g", "г", "gram", "grams"], Mass, 0.001),
        (&["kg", "кг"], Mass, 1.0),
        (&["t", "т", "ton", "tonne", "tonnes"], Mass, 1000.0),
        (&["oz", "ounce", "ounces", "унция", "унций"], Mass, 0.028_349_523_125),
        (&["lb", "lbs", "pound", "pounds", "фунт", "фунтов"], Mass, 0.453_592_37),
        (&["ml", "мл"], Volume, 0.001),
        (&["l", "л", "liter", "liters", "litre", "litres"], Volume, 1.0),
        (&["gal", "gallon", "gallons", "галлон"], Volume, 3.785_411_784),
        (&["qt", "quart", "quarts"], Volume, 0.946_352_946),
        (&["pt", "pint", "pints", "пинта"], Volume, 0.473_176_473),
        (&["cup", "cups"], Volume, 0.236_588_236_5),
        (&["floz", "fl oz"], Volume, 0.029_573_529_562_5),
        (&["ms", "мс"], Time, 0.001),
        (&["s", "sec", "secs", "second", "seconds", "с", "сек"], Time, 1.0),
        (&["min", "mins", "minute", "minutes", "мин"], Time, 60.0),
        (&["h", "hr", "hrs", "hour", "hours", "ч", "час", "часов"], Time, 3600.0),
        (&["d", "day", "days", "сут", "день", "дней", "дня"], Time, 86400.0),
        (&["wk", "week", "weeks", "нед", "неделя", "недель"], Time, 604_800.0),
        (&["yr", "year", "years", "год", "лет"], Time, 31_557_600.0),
        (&["b", "byte", "bytes", "байт"], Data, 1.0),
        (&["kb", "кб"], Data, 1e3),
        (&["mb", "мб"], Data, 1e6),
        (&["gb", "гб"], Data, 1e9),
        (&["tb", "тб"], Data, 1e12),
        (&["kib"], Data, 1024.0),
        (&["mib"], Data, 1_048_576.0),
        (&["gib"], Data, 1_073_741_824.0),
        (&["tib"], Data, 1_099_511_627_776.0),
        (&["m/s", "mps", "м/с"], Speed, 1.0),
        (&["km/h", "kmh", "kph", "км/ч"], Speed, 1.0 / 3.6),
        (&["mph"], Speed, 0.447_04),
        (&["kn", "knot", "knots", "узел", "узлов"], Speed, 0.514_444),
        (&["m2", "м2", "sqm"], Area, 1.0),
        (&["km2", "км2"], Area, 1e6),
        (&["ha", "га", "hectare", "hectares"], Area, 1e4),
        (&["acre", "acres", "акр"], Area, 4_046.856_422_4),
        (&["ft2", "sqft"], Area, 0.092_903_04),
    ]
};

/// Currency signs and words, for the codes they mean.
const CURRENCY_NAMES: &[(&str, &str)] = &[
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("₽", "RUB"),
    ("руб", "RUB"),
    ("рубль", "RUB"),
    ("рублей", "RUB"),
    ("р", "RUB"),
    ("доллар", "USD"),
    ("долларов", "USD"),
    ("евро", "EUR"),
    ("dollar", "USD"),
    ("dollars", "USD"),
    ("euro", "EUR"),
    ("euros", "EUR"),
];

/// A conversion asked for: so much of one unit, in another.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    amount: f64,
    from: String,
    to: String,
}

/// What a query came to.
pub enum Answer {
    /// The figure and the unit it is in.
    Value(String),
    /// Currencies, before their rates have come.
    Waiting,
    /// Two currencies of which one is not known.
    Unknown,
}

/// `<number> <unit> (to|in|в|->|=) <unit>`, or `None` for anything else.
pub fn parse(text: &str) -> Option<Query> {
    let text = text.trim().to_lowercase();
    let (left, to) = [" to ", " in ", " в ", " -> ", " = ", " into "].iter().find_map(|word| text.rsplit_once(word))?;
    let left = left.trim();
    // The number runs up to the first character that cannot be part of one;
    // the unit may follow with a space or without.
    let split = left.find(|c: char| !(c.is_ascii_digit() || ".,-+".contains(c))).unwrap_or(left.len());
    let (number, from) = left.split_at(split);
    // A sign in front: `$100`.
    let (number, from) = if number.is_empty() {
        let first = left.chars().next()?;
        (left[first.len_utf8()..].trim(), first.to_string())
    } else {
        (number, from.trim().to_owned())
    };
    let amount: f64 = number.replace(',', ".").parse().ok()?;
    let to = to.trim().to_owned();
    (!from.is_empty() && !to.is_empty() && amount.is_finite()).then_some(Query { amount, from, to })
}

fn unit(name: &str) -> Option<(Dimension, f64)> {
    let name = name.trim_end_matches('.');
    UNITS.iter().find(|(names, ..)| names.contains(&name)).map(|(_, dimension, size)| (*dimension, *size))
}

/// Kelvin from a temperature on the scale `name`, and back.
type Scale = fn(f64) -> f64;

fn kelvin(name: &str) -> Option<(Scale, Scale)> {
    match name.trim_start_matches('°') {
        "c" | "с" | "celsius" | "цельсия" => Some((|v| v + 273.15, |k| k - 273.15)),
        "f" | "fahrenheit" | "фаренгейта" => {
            Some((|v| (v - 32.0) * 5.0 / 9.0 + 273.15, |k| (k - 273.15) * 9.0 / 5.0 + 32.0))
        }
        "k" | "kelvin" | "кельвина" => Some((|v| v, |k| k)),
        _ => None,
    }
}

/// A currency code from a code, a sign or a word; `None` when it cannot be one.
pub fn currency(name: &str) -> Option<String> {
    if let Some((_, code)) = CURRENCY_NAMES.iter().find(|(word, _)| *word == name) {
        return Some((*code).to_owned());
    }
    (name.len() == 3 && name.chars().all(|c| c.is_ascii_alphabetic())).then(|| name.to_uppercase())
}

/// Whether the query is about money, so rates are worth fetching.
pub fn wants_rates(query: &Query) -> bool {
    unit(&query.from).is_none() && kelvin(&query.from).is_none() && currency(&query.from).is_some()
}

/// The answer, with `rates` per US dollar when there are any.
pub fn answer(query: &Query, rates: Option<&HashMap<String, f64>>) -> Option<Answer> {
    let Query { amount, from, to } = query;
    if let (Some((a, size_a)), Some((b, size_b))) = (unit(from), unit(to)) {
        return (a == b).then(|| Answer::Value(format!("{} {to}", figure(amount * size_a / size_b))));
    }
    // `c` is a centimetre short of a metre, not a unit; it is Celsius here.
    if let (Some((into, _)), Some((_, out))) = (kelvin(from), kelvin(to)) {
        let degrees = match to.trim_start_matches('°') {
            "k" | "kelvin" | "кельвина" => "K".to_owned(),
            short if short.chars().count() == 1 => format!("°{}", short.to_uppercase()),
            _ => to.clone(),
        };
        return Some(Answer::Value(format!("{} {degrees}", figure(out(into(*amount))))));
    }
    let (Some(a), Some(b)) = (currency(from), currency(to)) else { return None };
    let Some(rates) = rates else { return Some(Answer::Waiting) };
    let (Some(ra), Some(rb)) = (rates.get(&a), rates.get(&b)) else { return Some(Answer::Unknown) };
    Some(Answer::Value(format!("{} {b}", money(amount / ra * rb))))
}

/// A number without trailing zeros, to as many places as it is worth.
fn figure(value: f64) -> String {
    if value != 0.0 && (value.abs() >= 1e12 || value.abs() < 1e-4) {
        return format!("{value:.4e}");
    }
    let places = if value.abs() >= 100.0 { 2 } else { 6 };
    let text = format!("{value:.places$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    text.to_owned()
}

fn money(value: f64) -> String {
    if value.abs() >= 1.0 { format!("{value:.2}") } else { figure(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> String {
        match answer(&parse(text).unwrap(), None) {
            Some(Answer::Value(v)) => v,
            _ => panic!("no answer for {text}"),
        }
    }

    #[test]
    fn units() {
        assert_eq!(value("10 km in mi"), "6.213712 mi");
        assert_eq!(value("1,5 кг в г"), "1500 г");
        assert_eq!(value("1gib to mb"), "1073.74 mb");
        assert_eq!(value("100 c to f"), "212 °F");
        assert_eq!(value("0 °c in k"), "273.15 K");
    }

    #[test]
    fn money_waits_for_rates() {
        let query = parse("$100 to eur").unwrap();
        assert!(wants_rates(&query));
        assert!(matches!(answer(&query, None), Some(Answer::Waiting)));
        let rates = HashMap::from([("USD".to_owned(), 1.0), ("EUR".to_owned(), 0.5)]);
        assert!(matches!(answer(&query, Some(&rates)), Some(Answer::Value(v)) if v == "50.00 EUR"));
    }

    #[test]
    fn mismatched() {
        assert!(answer(&parse("10 km to kg").unwrap(), None).is_none());
        assert!(parse("hello world").is_none());
    }
}
