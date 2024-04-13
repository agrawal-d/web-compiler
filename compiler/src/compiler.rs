use crate::{
    chunk::Chunk,
    common::Opcode,
    interner::Interner,
    scanner::{Scanner, Token, TokenType},
    value::Value,
    xprint, xprintln,
};
use anyhow::*;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::{collections::HashMap, rc::Rc};

#[repr(u8)]
#[derive(Eq, Clone, Copy, TryFromPrimitive, PartialEq, PartialOrd, IntoPrimitive, strum_macros::Display)]
enum Precedence {
    None,
    Assignment, // =
    Or,         // or
    And,        // and
    Equality,   // == !=
    Comparison, // < > <= >=
    Term,       // + -
    Factor,     // * /
    Unary,      // ! -
    Call,       // . ()
    Primary,
}

type Parsefn<'src> = fn(&mut Compiler<'src>);

struct ParseRule<'src> {
    prefix: Option<Parsefn<'src>>,
    infix: Option<Parsefn<'src>>,
    precedence: Precedence,
}

/// This is a table that, given a token type, lets us find
/// 1. the function to compile a prefix expression starting with a token of that type,
/// 2. the function to compile an infix expression whose left operand is followed by a token of that type, and
/// 3. the precedence of an infix expression that uses that token as an operator.
fn get_rules<'src>() -> HashMap<TokenType, ParseRule<'src>> {
    let mut map = HashMap::new();
    use TokenType::*;

    macro_rules! add_rule {
        ($map: expr, $tokentype: expr, $prefix: expr, $infix: expr, $precedence: expr) => {
            $map.insert(
                $tokentype,
                ParseRule {
                    prefix: $prefix,
                    infix: $infix,
                    precedence: $precedence,
                },
            );
        };
    }

    add_rule!(map, LeftParen, Some(Compiler::grouping), None, Precedence::None);
    add_rule!(map, RightParen, None, None, Precedence::None);
    add_rule!(map, LeftBrace, None, None, Precedence::None);
    add_rule!(map, RightBrace, None, None, Precedence::None);
    add_rule!(map, Comma, None, None, Precedence::None);
    add_rule!(map, Dot, None, None, Precedence::None);
    add_rule!(map, Minus, Some(Compiler::unary), Some(Compiler::binary), Precedence::Term);
    add_rule!(map, Plus, None, Some(Compiler::binary), Precedence::Term);
    add_rule!(map, Semicolon, None, None, Precedence::None);
    add_rule!(map, Slash, None, Some(Compiler::binary), Precedence::Factor);
    add_rule!(map, Star, None, Some(Compiler::binary), Precedence::Factor);
    add_rule!(map, Bang, Some(Compiler::unary), None, Precedence::None);
    add_rule!(map, BangEqual, None, Some(Compiler::binary), Precedence::Equality);
    add_rule!(map, Equal, None, None, Precedence::None);
    add_rule!(map, EqualEqual, None, Some(Compiler::binary), Precedence::Equality);
    add_rule!(map, Greater, None, Some(Compiler::binary), Precedence::Comparison);
    add_rule!(map, GreaterEqual, None, Some(Compiler::binary), Precedence::Comparison);
    add_rule!(map, Less, None, Some(Compiler::binary), Precedence::Comparison);
    add_rule!(map, LessEqual, None, None, Precedence::Comparison);
    add_rule!(map, Identifier, None, None, Precedence::None);
    add_rule!(map, String, Some(Compiler::string), None, Precedence::None);
    add_rule!(map, Number, Some(Compiler::number), None, Precedence::None);
    add_rule!(map, And, None, None, Precedence::None);
    add_rule!(map, Class, None, None, Precedence::None);
    add_rule!(map, Else, None, None, Precedence::None);
    add_rule!(map, False, Some(Compiler::literal), None, Precedence::None);
    add_rule!(map, For, None, None, Precedence::None);
    add_rule!(map, Fun, None, None, Precedence::None);
    add_rule!(map, If, None, None, Precedence::None);
    add_rule!(map, Nil, Some(Compiler::literal), None, Precedence::None);
    add_rule!(map, Or, None, None, Precedence::None);
    add_rule!(map, Print, None, None, Precedence::None);
    add_rule!(map, Return, None, None, Precedence::None);
    add_rule!(map, Super, None, None, Precedence::None);
    add_rule!(map, This, None, None, Precedence::None);
    add_rule!(map, True, Some(Compiler::literal), None, Precedence::None);
    add_rule!(map, Var, None, None, Precedence::None);
    add_rule!(map, While, None, None, Precedence::None);
    add_rule!(map, Error, None, None, Precedence::None);
    add_rule!(map, EOF, None, None, Precedence::None);

    map
}

fn increment_prec(prec: Precedence) -> Precedence {
    (prec as u8 + 1).try_into().unwrap()
}

struct Parser {
    pub scanner: Scanner,
    pub current: Token,
    pub previous: Token,
    pub had_error: bool,
    pub panic_mode: bool,
}

impl Parser {
    fn new(scanner: Scanner) -> Parser {
        Parser {
            scanner,
            current: Token::new(),
            previous: Token::new(),
            had_error: false,
            panic_mode: false,
        }
    }

    fn error_at_current(&mut self, message: &str) {
        self.error_at(true, message);
    }

    fn error_at_previous(&mut self, message: &str) {
        self.error_at(false, message);
    }

    fn error_at(&mut self, current: bool, message: &str) {
        let current = if current { &self.current } else { &self.previous };

        if self.panic_mode {
            return;
        }

        self.panic_mode = true;
        xprint!(" [line {}] Error", current.line);

        if current.typ == TokenType::EOF {
            xprint!(" at end");
        } else if current.typ == TokenType::Error {
            // Nothing.
        } else {
            eprint!(" at '{}'", current.source);
        }

        xprintln!(": {}", message);
        self.had_error = true;
    }

    fn consume(&mut self, typ: TokenType, message: &str) {
        if self.current.typ == typ {
            self.advance();
            return;
        }

        self.error_at_current(message);
    }

    fn advance(&mut self) {
        self.previous = self.current.clone();

        loop {
            self.current = self.scanner.scan_token();
            if self.current.typ != TokenType::Error {
                break;
            }

            let current_source: Rc<str> = self.current.source.clone();
            self.error_at_current(current_source.as_ref());
        }
    }
}

pub struct Compiler<'src> {
    compiling_chunk: Chunk,
    parser: Parser,
    line: usize,
    interner: &'src mut Interner,
    rules: HashMap<TokenType, ParseRule<'src>>,
}

impl<'src> Compiler<'src> {
    pub fn compile(source: Rc<str>, interner: &mut Interner) -> Result<Chunk> {
        let line: usize = 0;
        let scanner: Scanner = Scanner::new(source);
        let parser = Parser::new(scanner);
        let rules = get_rules();
        let mut compiler = Compiler {
            compiling_chunk: Chunk::default(),
            line,
            parser,
            interner,
            rules,
        };

        compiler.parser.advance();
        compiler.expression();
        compiler.parser.consume(TokenType::EOF, "Expect end of expression.");
        compiler.end();
        Ok(compiler.compiling_chunk)
    }

    #[cfg(not(feature = "print_code"))]
    fn end(&mut self) {
        self.emit_return();
    }

    #[cfg(feature = "print_code")]
    fn end(&mut self) {
        self.emit_return();
        if !self.parser.had_error {
            self.compiling_chunk.disassemble("code", self.interner);
        }
    }

    fn get_rule(&self, token_type: TokenType) -> &ParseRule<'src> {
        self.rules.get(&token_type).unwrap()
    }

    fn binary(&mut self) {
        let operator_type = self.parser.previous.typ;
        let rule = self.get_rule(operator_type);
        self.parse_precedence(increment_prec(rule.precedence));

        match operator_type {
            TokenType::Plus => self.emit_byte(Opcode::Add as u8),
            TokenType::Minus => self.emit_byte(Opcode::Subtract as u8),
            TokenType::Star => self.emit_byte(Opcode::Multiply as u8),
            TokenType::Slash => self.emit_byte(Opcode::Divide as u8),
            TokenType::BangEqual => self.emit_bytes(Opcode::Equal as u8, Opcode::Not as u8),
            TokenType::EqualEqual => self.emit_byte(Opcode::Equal as u8),
            TokenType::Greater => self.emit_byte(Opcode::Greater as u8),
            TokenType::GreaterEqual => self.emit_bytes(Opcode::Less as u8, Opcode::Not as u8),
            TokenType::Less => self.emit_byte(Opcode::Less as u8),
            TokenType::LessEqual => self.emit_bytes(Opcode::Greater as u8, Opcode::Not as u8),
            _ => (),
        }
    }

    fn literal(&mut self) {
        match self.parser.previous.typ {
            TokenType::False => self.emit_byte(Opcode::False as u8),
            TokenType::Nil => self.emit_byte(Opcode::Nil as u8),
            TokenType::True => self.emit_byte(Opcode::True as u8),
            _ => (),
        }
    }

    fn expression(&mut self) {
        self.parse_precedence(Precedence::Assignment);
    }

    fn grouping(&mut self) {
        self.expression();
        self.parser.consume(TokenType::RightParen, "Expect ')' after expression.");
    }

    fn number(&mut self) {
        let num = self.parser.previous.source.parse::<f64>().unwrap();
        self.emit_constant(Value::Number(num));
    }

    fn string(&mut self) {
        let data = self.parser.previous.source.clone();
        let data = &data[1..data.len() - 1];
        let id = self.interner.intern(data);
        self.emit_constant(Value::Str(id));
    }

    fn unary(&mut self) {
        let operator_type = self.parser.previous.typ;
        self.parse_precedence(Precedence::Unary);

        match operator_type {
            TokenType::Minus => self.emit_byte(Opcode::Negate as u8),
            TokenType::Bang => self.emit_byte(Opcode::Not as u8),
            _ => (),
        }
    }

    // Parse expressions with equal or higher precedence
    fn parse_precedence(&mut self, precedence: Precedence) {
        self.parser.advance();
        let prefix_rule = self.get_rule(self.parser.previous.typ).prefix;

        match prefix_rule {
            Some(rule) => rule(self),
            None => {
                self.parser.error_at_previous("Expect expression");
                return;
            }
        }

        while precedence <= self.get_rule(self.parser.current.typ).precedence {
            self.parser.advance();
            let infix_rule = self.get_rule(self.parser.previous.typ).infix;

            match infix_rule {
                Some(rule) => rule(self),
                None => {
                    self.parser.error_at_previous("Expect expression");
                    return;
                }
            }
        }
    }

    fn make_constant(&mut self, value: Value) -> usize {
        self.compiling_chunk.add_constant(value)
    }

    fn emit_constant(&mut self, value: Value) {
        let index = self.compiling_chunk.add_constant(value);
        self.emit_bytes(Opcode::Constant as u8, index as u8);
    }

    fn emit_return(&mut self) {
        self.emit_byte(Opcode::Return as u8);
    }

    fn emit_byte(&mut self, byte: u8) {
        self.compiling_chunk.write_byte(byte, self.line);
    }

    fn emit_bytes(&mut self, byte1: u8, byte2: u8) {
        self.emit_byte(byte1);
        self.emit_byte(byte2);
    }
}
