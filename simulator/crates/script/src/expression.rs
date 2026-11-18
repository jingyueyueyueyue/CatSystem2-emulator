use crate::{Result, ScriptError};
use std::collections::BTreeMap;

const MAX_EXPRESSION_BYTES: usize = 64 * 1024;
const MAX_EXPRESSION_NODES: usize = 512;
const MAX_EXPRESSION_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct ExpressionState {
    pub shared: Vec<i32>,
    pub local: Vec<i32>,
    pub strings: BTreeMap<u32, Vec<u8>>,
    pub named: BTreeMap<String, BTreeMap<i32, i32>>,
    pub current: i32,
}

impl ExpressionState {
    pub fn new(shared_slots: usize) -> Self {
        Self {
            shared: vec![0; shared_slots],
            local: vec![0; 512],
            strings: BTreeMap::new(),
            named: BTreeMap::new(),
            current: 0,
        }
    }
}

impl Default for ExpressionState {
    fn default() -> Self {
        Self::new(2048)
    }
}

#[derive(Clone, Copy, Debug)]
enum Place {
    Shared(usize),
    Local(usize),
}

impl Place {
    fn get(self, state: &ExpressionState) -> i32 {
        match self {
            Self::Shared(index) => state.shared[index],
            Self::Local(index) => state.local[index],
        }
    }

    fn put(self, state: &mut ExpressionState, value: i32) {
        match self {
            Self::Shared(index) => state.shared[index] = value,
            Self::Local(index) => state.local[index] = value,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Value {
    Number(i32),
    Reference(Place),
}

impl Value {
    fn load(self, state: &ExpressionState) -> i32 {
        match self {
            Self::Number(value) => value,
            Self::Reference(place) => place.get(state),
        }
    }
}

#[derive(Clone, Debug)]
enum Node {
    Number(i32),
    Current,
    Variable(bool, Box<Node>, usize),
    Named(String, Option<Box<Node>>),
    Unary(&'static str, Box<Node>, usize),
    Binary(&'static str, Box<Node>, Box<Node>, usize),
    Postfix(bool, Box<Node>, usize),
    NoWrite(Box<Node>, Option<Box<Node>>),
}

#[derive(Clone, Debug)]
pub struct Expression(Node);

impl Expression {
    pub fn parse(source: &[u8]) -> Result<Self> {
        if source.len() > MAX_EXPRESSION_BYTES {
            return Err(expression_error(0, "expression is too long"));
        }
        let mut parser = Parser {
            source,
            position: 0,
            depth: 0,
            nodes: 0,
        };
        let node = parser.expression(1)?;
        parser.skip_space();
        if parser.position != source.len() {
            return Err(expression_error(
                parser.position,
                "expression has an unconsumed suffix",
            ));
        }
        Ok(Self(node))
    }

    pub fn evaluate(&self, state: &mut ExpressionState) -> Result<i32> {
        evaluate_root(&self.0, state)
    }
}

pub fn eval_expression(source: &[u8], state: &mut ExpressionState) -> Result<i32> {
    Expression::parse(source)?.evaluate(state)
}

pub fn eval_expression_parameter(source: &[u8], state: &mut ExpressionState) -> Result<i32> {
    if source.len() > MAX_EXPRESSION_BYTES {
        return Err(expression_error(0, "expression is too long"));
    }
    eval_expression_fallback_with_text(source.strip_suffix(b"%").unwrap_or(source), state)
}

pub(crate) fn eval_expression_fallback_with_text(
    source: &[u8],
    state: &mut ExpressionState,
) -> Result<i32> {
    if source.len() > MAX_EXPRESSION_BYTES {
        return Err(expression_error(0, "expression is too long"));
    }
    // A high byte outside quoted text aborts the expression before any
    // assignment is evaluated. Quoted text remains a numeric zero operand.
    let mut quote = None;
    for &byte in source {
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if matches!(byte, b'"' | b'\'') {
            quote = Some(byte);
        } else if byte >= 0x80 {
            return Ok(0);
        }
    }
    eval_expression_fallback(source, state)
}

pub fn eval_expression_fallback(source: &[u8], state: &mut ExpressionState) -> Result<i32> {
    if source.len() > MAX_EXPRESSION_BYTES {
        return Err(expression_error(0, "expression is too long"));
    }
    let node = FallbackParser::parse(source)?;
    evaluate_root(&node, state)
}

fn expression_error(offset: usize, message: impl Into<String>) -> ScriptError {
    ScriptError::InvalidExpression { offset, message: message.into() }
}

fn evaluate_root(node: &Node, state: &mut ExpressionState) -> Result<i32> {
    let value = evaluate(node, state)?.load(state);
    if matches!(node, Node::Unary("+", ..)) {
        Ok(0)
    } else {
        Ok(value)
    }
}

fn evaluate(node: &Node, state: &mut ExpressionState) -> Result<Value> {
    evaluate_access(node, state, "read")
}

fn evaluate_access(node: &Node, state: &mut ExpressionState, access: &'static str) -> Result<Value> {
    Ok(match node {
        Node::Number(value) => Value::Number(*value),
        Node::Current => Value::Number(state.current),
        Node::Named(name, index) => {
            let bank = index
                .as_ref()
                .map(|index| evaluate(index, state).map(|value| value.load(state)))
                .transpose()?
                .unwrap_or(0);
            Value::Number(
                state
                    .named
                    .get(name)
                    .and_then(|banks| banks.get(&bank))
                    .copied()
                    .unwrap_or(0),
            )
        }
        Node::Variable(local, index, at) => {
            let index = evaluate(index, state)?.load(state);
            let storage = if *local { &state.local } else { &state.shared };
            if index < 0 || index as usize >= storage.len() {
                let domain = if *local { "local" } else { "shared" };
                return Err(expression_error(*at, format!(
                    "variable index is out of bounds: domain={domain} access={access} index={index} capacity={}",
                    storage.len(),
                )));
            }
            Value::Reference(if *local {
                Place::Local(index as usize)
            } else {
                Place::Shared(index as usize)
            })
        }
        Node::Unary(operator, argument, at) => {
            let access = if matches!(*operator, "++" | "--") { "read/write" } else { "read" };
            let value = evaluate_access(argument, state, access)?;
            let number = value.load(state);
            let result = match *operator {
                "+" => number,
                "-" => number.wrapping_neg(),
                "!" => i32::from(number == 0),
                "~" => !number,
                "++" | "--" => {
                    let Value::Reference(place) = value else {
                        return Err(expression_error(*at, "increment requires a variable"));
                    };
                    let result = number.wrapping_add(if *operator == "++" { 1 } else { -1 });
                    place.put(state, result);
                    result
                }
                _ => unreachable!(),
            };
            Value::Number(result)
        }
        Node::Binary(operator, left, right, at) => {
            let operator = *operator;
            let access = if operator == "=" { "write" }
                else if is_assignment(operator) { "read/write" } else { "read" };
            let left = evaluate_access(left, state, access)?;
            let right = evaluate(right, state)?.load(state);
            if is_assignment(operator) {
                let Value::Reference(place) = left else {
                    return Err(expression_error(*at, "assignment requires a variable"));
                };
                let result = if operator == "=" {
                    right
                } else {
                    binary(
                        &operator[..operator.len() - 1],
                        place.get(state),
                        right,
                        true,
                        *at,
                    )?
                };
                place.put(state, result);
                Value::Number(result)
            } else {
                Value::Number(binary(operator, left.load(state), right, false, *at)?)
            }
        }
        Node::Postfix(increment, argument, at) => {
            let Value::Reference(place) = evaluate_access(argument, state, "read/write")? else {
                return Err(expression_error(*at, "increment requires a variable"));
            };
            let value = place.get(state);
            place.put(state, value.wrapping_add(if *increment { 1 } else { -1 }));
            Value::Number(value)
        }
        Node::NoWrite(left, right) => {
            evaluate(left, state)?;
            if let Some(right) = right {
                evaluate(right, state)?;
            }
            Value::Number(0)
        }
    })
}

fn is_assignment(operator: &str) -> bool {
    matches!(
        operator,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>="
    )
}

fn binary(operator: &str, left: i32, right: i32, compound: bool, at: usize) -> Result<i32> {
    Ok(match operator {
        "+" => left.wrapping_add(right),
        "-" => left.wrapping_sub(right),
        "*" => left.wrapping_mul(right),
        "/" | "%" => {
            if right == 0 && !compound {
                0
            } else if right == 0 || (left == i32::MIN && right == -1) {
                return Err(expression_error(at, "integer division failed"));
            } else if operator == "/" {
                left / right
            } else {
                left % right
            }
        }
        "<<" => left.wrapping_shl(right as u32),
        ">>" => left.wrapping_shr(right as u32),
        "<" => i32::from(left < right),
        "<=" => i32::from(left <= right),
        ">" => i32::from(left > right),
        ">=" => i32::from(left >= right),
        "==" => i32::from(left == right),
        "!=" => i32::from(left != right),
        "&" => left & right,
        "^" => left ^ right,
        "|" => left | right,
        "&&" => i32::from(left != 0 && right != 0),
        "||" => i32::from(left != 0 || right != 0),
        _ => return Err(expression_error(at, "unsupported expression operator")),
    })
}

struct Parser<'a> {
    source: &'a [u8],
    position: usize,
    depth: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn count_node(&mut self) -> Result<()> {
        self.nodes += 1;
        if self.nodes > MAX_EXPRESSION_NODES {
            return Err(expression_error(self.position, "too many expression nodes"));
        }
        Ok(())
    }

    fn skip_space(&mut self) {
        while self
            .source
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }

    fn take(&mut self, token: &[u8]) -> bool {
        self.skip_space();
        if self
            .source
            .get(self.position..)
            .is_some_and(|source| source.starts_with(token))
        {
            self.position += token.len();
            true
        } else {
            false
        }
    }

    fn expression(&mut self, minimum: u8) -> Result<Node> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(expression_error(
                self.position,
                "expression is nested too deeply",
            ));
        }
        let result = (|| {
            let mut left = self.primary()?;
            if minimum <= 12 {
                for operator in ["++", "--"] {
                    if self.take(operator.as_bytes()) {
                        left = if matches!(&left, Node::Unary("+", ..)) {
                            Node::NoWrite(Box::new(left), None)
                        } else {
                            Node::Unary(
                                if operator == "++" { "++" } else { "--" },
                                Box::new(left),
                                self.position - operator.len(),
                            )
                        };
                        break;
                    }
                }
            }
            loop {
                self.skip_space();
                let Some(operator) = self.operator() else {
                    break;
                };
                let precedence = precedence(operator);
                if precedence < minimum {
                    break;
                }
                let at = self.position;
                self.position += operator.len();
                let right = self.expression(if is_assignment(operator) {
                    precedence
                } else {
                    precedence + 1
                })?;
                self.count_node()?;
                left = Node::Binary(operator, Box::new(left), Box::new(right), at);
            }
            Ok(left)
        })();
        self.depth -= 1;
        result
    }

    fn operator(&self) -> Option<&'static str> {
        [
            "<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "==", "!=", "<=", ">=",
            "&&", "||", "<<", ">>", "=", "+", "-", "*", "/", "%", "&", "|", "^", "<", ">",
        ]
        .into_iter()
        .find(|operator| {
            self.source
                .get(self.position..)
                .is_some_and(|source| source.starts_with(operator.as_bytes()))
        })
    }

    fn primary(&mut self) -> Result<Node> {
        self.count_node()?;
        self.skip_space();
        let at = self.position;
        for operator in ["++", "--", "+", "-", "!", "~"] {
            if self.take(operator.as_bytes()) {
                return Ok(Node::Unary(
                    match operator {
                        "++" => "++",
                        "--" => "--",
                        "+" => "+",
                        "-" => "-",
                        "!" => "!",
                        "~" => "~",
                        _ => unreachable!(),
                    },
                    Box::new(self.expression(if operator == "+" { 13 } else { 12 })?),
                    at,
                ));
            }
        }
        if self.take(b"(") {
            let node = self.expression(1)?;
            if !self.take(b")") {
                return Err(expression_error(self.position, "missing right parenthesis"));
            }
            return Ok(node);
        }
        if self.take(b"@") {
            return Ok(Node::Current);
        }
        if let Some(&prefix @ (b'#' | b'$' | b'\\')) = self.source.get(self.position) {
            self.position += 1;
            let index = if self.take(b"(") {
                let node = self.expression(1)?;
                if !self.take(b")") {
                    return Err(expression_error(
                        self.position,
                        "variable index is missing a right parenthesis",
                    ));
                }
                node
            } else {
                Node::Number(self.number(false)?)
            };
            return Ok(Node::Variable(prefix == b'\\', Box::new(index), at));
        }
        if self
            .source
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            let start = self.position;
            while self
                .source
                .get(self.position)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                self.position += 1;
            }
            let name = String::from_utf8(self.source[start..self.position].to_vec())
                .map_err(|_| expression_error(start, "identifier is not ASCII"))?;
            let bank = if self.take(b"[") {
                let index = self.expression(1)?;
                if !self.take(b"]") {
                    return Err(expression_error(
                        self.position,
                        "named value index is missing a right bracket",
                    ));
                }
                Some(Box::new(index))
            } else {
                None
            };
            return Ok(Node::Named(name, bank));
        }
        Ok(Node::Number(self.number(true)?))
    }

    fn number(&mut self, allow_hex: bool) -> Result<i32> {
        self.skip_space();
        let start = self.position;
        let radix = if allow_hex
            && self
                .source
                .get(self.position..)
                .is_some_and(|source| source.starts_with(b"0x") || source.starts_with(b"0X"))
        {
            self.position += 2;
            16
        } else {
            10
        };
        let digits = self.position;
        let mut value = 0u32;
        while let Some(&byte) = self.source.get(self.position) {
            let Some(digit) = (byte as char).to_digit(radix) else {
                break;
            };
            value = value.wrapping_mul(radix).wrapping_add(digit);
            self.position += 1;
        }
        if self.position == digits {
            return Err(expression_error(start, "expected an integer or variable"));
        }
        Ok(value as i32)
    }
}

fn precedence(operator: &str) -> u8 {
    match operator {
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" => 1,
        "||" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" => 7,
        "<" | ">" | "<=" | ">=" => 8,
        "<<" | ">>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        _ => unreachable!(),
    }
}

#[derive(Clone, Copy)]
enum FallbackOperator {
    Binary(&'static str),
    Unary(&'static str),
    Increment(bool, bool),
    Variable(bool),
    Open(bool),
}

impl FallbackOperator {
    fn priority(self) -> (i32, i32) {
        match self {
            Self::Binary(operator) => match operator {
                "*" | "/" | "%" => (27, 26),
                "+" | "-" => (25, 24),
                "<<" | ">>" => (23, 22),
                "<" | "<=" | ">" | ">=" => (21, 20),
                "==" | "!=" => (19, 18),
                "&" => (17, 16),
                "^" => (15, 14),
                "|" => (13, 12),
                "&&" => (11, 10),
                "||" => (9, 8),
                operator if is_assignment(operator) => (4, 5),
                _ => (0, 0),
            },
            Self::Unary("~") => (0, 29),
            Self::Unary(_) | Self::Increment(_, true) => (28, 29),
            Self::Increment(_, false) => (29, 28),
            Self::Variable(_) => (30, 31),
            Self::Open(_) => (0, 33),
        }
    }
}

#[derive(Clone, Copy)]
enum Previous {
    Operator,
    Name,
    Value,
    Closing,
}

struct FallbackParser;

impl FallbackParser {
    fn parse(source: &[u8]) -> Result<Node> {
        let mut cursor = 0;
        let mut values = Vec::new();
        let mut operators = Vec::<(FallbackOperator, usize)>::new();
        let mut previous = Previous::Operator;
        let mut nodes = 0;
        while cursor < source.len() {
            if source[cursor].is_ascii_whitespace() {
                cursor += 1;
                continue;
            }
            nodes += 1;
            if nodes > MAX_EXPRESSION_NODES {
                return Err(expression_error(cursor, "too many expression nodes"));
            }
            let at = cursor;
            let byte = source[cursor];
            if byte.is_ascii_digit() {
                let mut parser = Parser {
                    source,
                    position: cursor,
                    depth: 0,
                    nodes: 0,
                };
                values.push(Node::Number(parser.number(true)?));
                cursor = parser.position;
                previous = Previous::Value;
                continue;
            }
            if byte.is_ascii_alphabetic() || byte == b'_' {
                cursor += 1;
                while source
                    .get(cursor)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    cursor += 1;
                }
                let name = String::from_utf8(source[at..cursor].to_vec())
                    .map_err(|_| expression_error(at, "identifier is not ASCII"))?;
                values.push(Node::Named(name, None));
                previous = Previous::Name;
                continue;
            }
            if matches!(byte, b'"' | b'\'') {
                cursor += 1;
                while source.get(cursor).is_some_and(|&value| value != byte) {
                    cursor += 1;
                }
                if cursor == source.len() {
                    return Err(expression_error(at, "string literal is not terminated"));
                }
                cursor += 1;
                values.push(Node::Number(0));
                previous = Previous::Value;
                continue;
            }
            if byte == b'@' {
                values.push(Node::Current);
                cursor += 1;
                previous = Previous::Value;
                continue;
            }
            if matches!(byte, b'#' | b'$' | b'\\') {
                cursor += 1;
                operators.push((FallbackOperator::Variable(byte == b'\\'), at));
                previous = Previous::Operator;
                continue;
            }
            if matches!(byte, b')' | b']') {
                let indexed = byte == b']';
                loop {
                    let (operator, position) = operators
                        .pop()
                        .ok_or_else(|| expression_error(at, "unmatched expression bracket"))?;
                    if let FallbackOperator::Open(is_index) = operator {
                        if is_index != indexed {
                            return Err(expression_error(at, "unmatched expression bracket"));
                        }
                        break;
                    }
                    Self::reduce(operator, position, &mut values)?;
                }
                if indexed {
                    let bank = values
                        .pop()
                        .ok_or_else(|| expression_error(at, "missing index expression"))?;
                    match values.last_mut() {
                        Some(Node::Named(_, target)) => *target = Some(Box::new(bank)),
                        Some(Node::Number(_)) => {}
                        _ => return Err(expression_error(at, "indexed target is unsupported")),
                    }
                }
                cursor += 1;
                previous = Previous::Closing;
                continue;
            }
            let operator = match byte {
                b'(' | b'[' => {
                    cursor += 1;
                    FallbackOperator::Open(byte == b'[')
                }
                _ => {
                    let text = [
                        "<<=", ">>=", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
                        "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "=", "+", "-", "*", "/",
                        "%", "&", "|", "^", "<", ">", "!", "~", ",", ".", "?", ":",
                    ]
                    .into_iter()
                    .find(|text| source[cursor..].starts_with(text.as_bytes()))
                    .ok_or_else(|| expression_error(at, "unsupported expression token"))?;
                    cursor += text.len();
                    match text {
                        "++" | "--" => FallbackOperator::Increment(
                            text == "++",
                            matches!(previous, Previous::Name | Previous::Closing),
                        ),
                        "+" | "-" if matches!(previous, Previous::Operator) => {
                            FallbackOperator::Unary(text)
                        }
                        "!" | "~" => FallbackOperator::Unary(text),
                        _ => FallbackOperator::Binary(text),
                    }
                }
            };
            while operators.last().is_some_and(|(top, _)| {
                !matches!(top, FallbackOperator::Open(_))
                    && top.priority().0 >= operator.priority().1
            }) {
                let (top, position) = operators.pop().unwrap();
                Self::reduce(top, position, &mut values)?;
            }
            operators.push((operator, at));
            previous = Previous::Operator;
        }
        while let Some((operator, at)) = operators.pop() {
            if matches!(operator, FallbackOperator::Open(_)) {
                return Err(expression_error(at, "unmatched expression bracket"));
            }
            Self::reduce(operator, at, &mut values)?;
        }
        Ok(values.pop().unwrap_or(Node::Number(0)))
    }

    fn reduce(operator: FallbackOperator, at: usize, values: &mut Vec<Node>) -> Result<()> {
        let right = values
            .pop()
            .ok_or_else(|| expression_error(at, "expression operand is missing"))?;
        let node = match operator {
            FallbackOperator::Variable(local) => Node::Variable(local, Box::new(right), at),
            FallbackOperator::Unary(operator) => Node::Unary(operator, Box::new(right), at),
            FallbackOperator::Increment(increment, postfix) => {
                if !matches!(right, Node::Variable(..)) {
                    Node::NoWrite(Box::new(right), None)
                } else if postfix {
                    Node::Postfix(increment, Box::new(right), at)
                } else {
                    Node::Unary(if increment { "++" } else { "--" }, Box::new(right), at)
                }
            }
            FallbackOperator::Binary(operator) => {
                let left = values
                    .pop()
                    .ok_or_else(|| expression_error(at, "expression operand is missing"))?;
                if matches!(operator, "," | "." | "?" | ":") {
                    Node::Number(0)
                } else if is_assignment(operator) && !matches!(left, Node::Variable(..)) {
                    Node::NoWrite(Box::new(left), Some(Box::new(right)))
                } else {
                    Node::Binary(operator, Box::new(left), Box::new(right), at)
                }
            }
            FallbackOperator::Open(_) => unreachable!(),
        };
        values.push(node);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_preserves_assignment_target_and_precedence() {
        let mut state = ExpressionState::new(4);
        assert_eq!(eval_expression(b"#0=1+#0++", &mut state).unwrap(), 2);
        assert_eq!(state.shared[0], 2);
        assert_eq!(eval_expression(b"1||(++#1)", &mut state).unwrap(), 1);
        assert_eq!(state.shared[1], 1);
        assert_eq!(eval_expression(b"0x10>>2", &mut state).unwrap(), 4);
    }

    #[test]
    fn expression_reports_invalid_indices_and_division() {
        let mut state = ExpressionState::new(1);
        assert!(matches!(
            eval_expression(b"#2", &mut state),
            Err(ScriptError::InvalidExpression { .. })
        ));
        assert!(matches!(
            eval_expression(b"#0/=0", &mut state),
            Err(ScriptError::InvalidExpression { .. })
        ));
        state.shared[0] = 5;
        assert_eq!(eval_expression_parameter(b"(#0=2)+\x81\x40", &mut state).unwrap(), 0);
        assert_eq!(state.shared[0], 5);
        assert_eq!(eval_expression_parameter(b"\"\x81\x40\"+7%", &mut state).unwrap(), 7);
        assert_eq!(eval_expression_parameter(b"1+(2 3)", &mut state).unwrap(), 5);
        assert!(eval_expression(b"1+(2 3)", &mut state).is_err());
    }

    #[test]
    fn fallback_keeps_only_the_final_expression_result() {
        let mut state = ExpressionState::new(2);
        state.shared[0] = 7;
        state.shared[1] = 11;
        assert_eq!(
            eval_expression_fallback(b"missing ($0+=1) ($1+=2)", &mut state).unwrap(),
            13
        );
        assert_eq!(state.shared, [7, 13]);
    }
}
