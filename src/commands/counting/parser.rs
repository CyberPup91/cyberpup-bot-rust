struct Parser<'a> {
    input: &'a [u8],
    offset: usize,
}

pub fn evaluate(input: &str) -> Option<f64> {
    let mut parser = Parser {
        input: input.as_bytes(),
        offset: 0,
    };
    let value = parser.expression()?;
    parser.whitespace();
    (parser.offset == parser.input.len() && value.is_finite()).then_some(value)
}

impl Parser<'_> {
    fn whitespace(&mut self) {
        while self
            .input
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }

    fn expression(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        loop {
            self.whitespace();
            match self.input.get(self.offset).copied() {
                Some(b'+') => {
                    self.offset += 1;
                    value += self.product()?;
                }
                Some(b'-') => {
                    self.offset += 1;
                    value -= self.product()?;
                }
                _ => return value.is_finite().then_some(value),
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            self.whitespace();
            match self.input.get(self.offset).copied() {
                Some(b'*') => {
                    self.offset += 1;
                    value *= self.unary()?;
                }
                Some(b'/') => {
                    self.offset += 1;
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return None;
                    }
                    value /= divisor;
                }
                Some(b'%') => {
                    self.offset += 1;
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return None;
                    }
                    value %= divisor;
                }
                _ => return value.is_finite().then_some(value),
            }
            if !value.is_finite() {
                return None;
            }
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        self.whitespace();
        if self.input.get(self.offset) == Some(&b'^') {
            self.offset += 1;
        } else if self.input.get(self.offset..self.offset + 2) == Some(b"**") {
            self.offset += 2;
        } else {
            return Some(base);
        }
        let exponent = self.unary()?;
        let value = base.powf(exponent);
        value.is_finite().then_some(value)
    }

    fn unary(&mut self) -> Option<f64> {
        self.whitespace();
        match self.input.get(self.offset).copied() {
            Some(b'+') => {
                self.offset += 1;
                self.unary()
            }
            Some(b'-') => {
                self.offset += 1;
                Some(-self.unary()?)
            }
            _ => self.power(),
        }
    }

    fn primary(&mut self) -> Option<f64> {
        self.whitespace();
        if self.input.get(self.offset) == Some(&b'(') {
            self.offset += 1;
            let value = self.expression()?;
            self.whitespace();
            if self.input.get(self.offset) != Some(&b')') {
                return None;
            }
            self.offset += 1;
            return Some(value);
        }

        let start = self.offset;
        let mut saw_digit = false;
        while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
            saw_digit = true;
            self.offset += 1;
        }
        if self.input.get(self.offset) == Some(&b'.') {
            self.offset += 1;
            while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
                saw_digit = true;
                self.offset += 1;
            }
        }
        if !saw_digit {
            return None;
        }
        std::str::from_utf8(&self.input[start..self.offset])
            .ok()?
            .parse()
            .ok()
    }
}
