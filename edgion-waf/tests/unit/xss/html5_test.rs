use super::*;

#[test]
fn high_byte_is_data_in_all_whitespace_skip_states() {
    for input in [
        b"<a \xffid=x><script>".as_slice(),
        b"<a id \xff=x><script>".as_slice(),
        b"<a id= \xff><script>".as_slice(),
    ] {
        let mut html = Html5State::new(input, Html5Flags::DataState);
        let mut found_script = false;
        let mut tokens = 0;
        while html.next() {
            tokens += 1;
            assert!(tokens <= input.len() * 2);
            if html.token_type == TokenType::TagNameOpen && html.token_start.starts_with(b"script") {
                found_script = true;
            }
        }
        assert!(found_script, "tokenizer stopped before script for {input:?}");
    }
}

#[test]
fn eof_in_all_whitespace_skip_states_terminates() {
    for input in [b"<a ".as_slice(), b"<a id ".as_slice(), b"<a id= ".as_slice()] {
        let mut html = Html5State::new(input, Html5Flags::DataState);
        let mut tokens = 0;
        while html.next() {
            tokens += 1;
            assert!(tokens <= input.len() * 2);
        }
        assert_eq!(html.position(), input.len());
    }
}

impl<'a> Html5State<'a> {
    #[cfg(test)]
    pub fn debug_is_close(&self) -> bool {
        self.is_close
    }
    #[cfg(test)]
    pub fn debug_pos(&self) -> usize {
        self.pos
    }
}
