//! A tiny arithmetic expression language for the `Expression` node.
//!
//! Grammar (recursive descent, standard precedence):
//!
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := unary (('*' | '/') unary)*
//! unary   := '-' unary | power
//! power   := atom ('^' unary)?            (right associative)
//! atom    := number | ident | ident '(' args ')' | '(' expr ')'
//! ```
//!
//! Variables: `r g b a x y w h` (pixel color and coordinates, image size).
//! Functions: `pow min max abs sqrt clamp floor ceil sin cos exp log lerp step smoothstep`.

use crate::error::{Error, Result};

/// Parsed expression tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f32),
    Var(Var),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

/// Per-pixel variables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    R,
    G,
    B,
    A,
    X,
    Y,
    W,
    H,
}

/// Binary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

/// Built-in functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Pow,
    Min,
    Max,
    Abs,
    Sqrt,
    Clamp,
    Floor,
    Ceil,
    Sin,
    Cos,
    Exp,
    Log,
    Lerp,
    Step,
    Smoothstep,
}

impl Func {
    fn parse(name: &str) -> Option<(Func, usize)> {
        Some(match name {
            "pow" => (Func::Pow, 2),
            "min" => (Func::Min, 2),
            "max" => (Func::Max, 2),
            "abs" => (Func::Abs, 1),
            "sqrt" => (Func::Sqrt, 1),
            "clamp" => (Func::Clamp, 3),
            "floor" => (Func::Floor, 1),
            "ceil" => (Func::Ceil, 1),
            "sin" => (Func::Sin, 1),
            "cos" => (Func::Cos, 1),
            "exp" => (Func::Exp, 1),
            "log" => (Func::Log, 1),
            "lerp" => (Func::Lerp, 3),
            "step" => (Func::Step, 2),
            "smoothstep" => (Func::Smoothstep, 3),
            _ => return None,
        })
    }
}

/// Values of the variables for one pixel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Env {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Env {
    /// Environment for a pixel of an image.
    pub fn pixel(x: usize, y: usize, w: usize, h: usize, rgba: [f32; 4]) -> Self {
        Env {
            r: rgba[0],
            g: rgba[1],
            b: rgba[2],
            a: rgba[3],
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
        }
    }
}

impl Expr {
    /// Parse an expression string.
    pub fn parse(src: &str) -> Result<Expr> {
        let tokens = tokenize(src)?;
        let mut p = Parser { tokens, pos: 0 };
        let e = p.expr()?;
        if p.pos != p.tokens.len() {
            return Err(Error::Expr(format!(
                "unexpected token {:?}",
                p.tokens[p.pos]
            )));
        }
        Ok(e)
    }

    /// Evaluate against an environment.
    pub fn eval(&self, env: &Env) -> f32 {
        match self {
            Expr::Num(n) => *n,
            Expr::Var(v) => match v {
                Var::R => env.r,
                Var::G => env.g,
                Var::B => env.b,
                Var::A => env.a,
                Var::X => env.x,
                Var::Y => env.y,
                Var::W => env.w,
                Var::H => env.h,
            },
            Expr::Neg(e) => -e.eval(env),
            Expr::Bin(op, l, r) => {
                let a = l.eval(env);
                let b = r.eval(env);
                match op {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    Op::Pow => a.powf(b),
                }
            }
            Expr::Call(f, args) => {
                let v: Vec<f32> = args.iter().map(|a| a.eval(env)).collect();
                match f {
                    Func::Pow => v[0].powf(v[1]),
                    Func::Min => v[0].min(v[1]),
                    Func::Max => v[0].max(v[1]),
                    Func::Abs => v[0].abs(),
                    Func::Sqrt => v[0].max(0.0).sqrt(),
                    Func::Clamp => {
                        let (lo, hi) = if v[1] <= v[2] {
                            (v[1], v[2])
                        } else {
                            (v[2], v[1])
                        };
                        v[0].clamp(lo, hi)
                    }
                    Func::Floor => v[0].floor(),
                    Func::Ceil => v[0].ceil(),
                    Func::Sin => v[0].sin(),
                    Func::Cos => v[0].cos(),
                    Func::Exp => v[0].exp(),
                    Func::Log => {
                        if v[0] > 0.0 {
                            v[0].ln()
                        } else {
                            f32::NEG_INFINITY
                        }
                    }
                    Func::Lerp => v[0] + (v[1] - v[0]) * v[2],
                    Func::Step => {
                        if v[1] < v[0] {
                            0.0
                        } else {
                            1.0
                        }
                    }
                    Func::Smoothstep => {
                        let (e0, e1, x) = (v[0], v[1], v[2]);
                        if e1 == e0 {
                            if x < e0 {
                                0.0
                            } else {
                                1.0
                            }
                        } else {
                            let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
                            t * t * (3.0 - 2.0 * t)
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f32),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Comma,
}

fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '^' => {
                out.push(Tok::Caret);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let save = i;
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    if i < chars.len() && chars[i].is_ascii_digit() {
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    } else {
                        i = save;
                    }
                }
                let s: String = chars[start..i].iter().collect();
                let n: f32 = s
                    .parse()
                    .map_err(|_| Error::Expr(format!("bad number `{s}`")))?;
                out.push(Tok::Num(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => {
                return Err(Error::Expr(format!(
                    "unexpected character `{other}` at {i}"
                )))
            }
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn expect(&mut self, t: Tok) -> Result<()> {
        match self.next() {
            Some(ref got) if *got == t => Ok(()),
            got => Err(Error::Expr(format!("expected {t:?}, got {got:?}"))),
        }
    }

    fn expr(&mut self) -> Result<Expr> {
        let mut lhs = self.term()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => Op::Add,
                Some(Tok::Minus) => Op::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.term()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => Op::Mul,
                Some(Tok::Slash) => Op::Div,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.unary()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.peek() == Some(&Tok::Minus) {
            self.pos += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr> {
        let base = self.atom()?;
        if self.peek() == Some(&Tok::Caret) {
            self.pos += 1;
            let exp = self.unary()?;
            return Ok(Expr::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Expr> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                self.expect(Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let (f, arity) = Func::parse(&name)
                        .ok_or_else(|| Error::Expr(format!("unknown function `{name}`")))?;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if self.peek() == Some(&Tok::Comma) {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(Tok::RParen)?;
                    if args.len() != arity {
                        return Err(Error::Expr(format!(
                            "`{name}` takes {arity} argument(s), got {}",
                            args.len()
                        )));
                    }
                    Ok(Expr::Call(f, args))
                } else {
                    let v = match name.as_str() {
                        "r" => Var::R,
                        "g" => Var::G,
                        "b" => Var::B,
                        "a" => Var::A,
                        "x" => Var::X,
                        "y" => Var::Y,
                        "w" => Var::W,
                        "h" => Var::H,
                        "pi" => return Ok(Expr::Num(std::f32::consts::PI)),
                        _ => return Err(Error::Expr(format!("unknown variable `{name}`"))),
                    };
                    Ok(Expr::Var(v))
                }
            }
            other => Err(Error::Expr(format!("unexpected {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f32 {
        let env = Env {
            r: 0.5,
            g: 0.25,
            b: 1.0,
            a: 1.0,
            x: 3.0,
            y: 4.0,
            w: 10.0,
            h: 20.0,
        };
        Expr::parse(s).unwrap().eval(&env)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert!((ev("r*0.5+g") - 0.5).abs() < 1e-6);
        assert!((ev("1+2*3") - 7.0).abs() < 1e-6);
        assert!((ev("(1+2)*3") - 9.0).abs() < 1e-6);
        assert!((ev("2^3^2") - 512.0).abs() < 1e-3);
        assert!((ev("-2^2") - -4.0).abs() < 1e-6);
        assert!((ev("2*-3") - -6.0).abs() < 1e-6);
        assert!((ev("10/4") - 2.5).abs() < 1e-6);
        assert!((ev("x/w + y/h") - 0.5).abs() < 1e-6);
        assert_eq!(ev("1/0"), 0.0);
    }

    #[test]
    fn functions() {
        assert!((ev("pow(2,10)") - 1024.0).abs() < 1e-3);
        assert!((ev("min(r,g)") - 0.25).abs() < 1e-6);
        assert!((ev("max(r, g)") - 0.5).abs() < 1e-6);
        assert!((ev("abs(-3.5)") - 3.5).abs() < 1e-6);
        assert!((ev("clamp(5, 0, 1)") - 1.0).abs() < 1e-6);
        assert!((ev("smoothstep(0,1,0.5)") - 0.5).abs() < 1e-6);
        assert!((ev("lerp(0, 10, 0.25)") - 2.5).abs() < 1e-6);
        assert!((ev("sqrt(16)") - 4.0).abs() < 1e-6);
        assert!((ev("1e2") - 100.0).abs() < 1e-6);
    }

    #[test]
    fn errors() {
        assert!(Expr::parse("r +").is_err());
        assert!(Expr::parse("(r").is_err());
        assert!(Expr::parse("foo(1)").is_err());
        assert!(Expr::parse("q").is_err());
        assert!(Expr::parse("min(1)").is_err());
        assert!(Expr::parse("1 $ 2").is_err());
        assert!(Expr::parse("1 2").is_err());
    }
}
