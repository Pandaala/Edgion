// This module includes the pinned, checked-in SQL data.

// The committed table is pinned to libinjection-go v0.3.2.
include!("generated_data.rs");

// Additional helper functions can be added here
impl CharType {
    pub fn is_white(&self) -> bool {
        matches!(self, CharType::White)
    }

    pub fn is_word(&self) -> bool {
        matches!(self, CharType::Word | CharType::BWord)
    }

    pub fn is_string_start(&self) -> bool {
        matches!(
            self,
            CharType::String
                | CharType::BString
                | CharType::EString
                | CharType::NQString
                | CharType::QString
                | CharType::UString
                | CharType::XString
        )
    }

    pub fn is_operator(&self) -> bool {
        matches!(self, CharType::Op1 | CharType::Op2 | CharType::Unary)
    }

    pub fn is_number(&self) -> bool {
        matches!(self, CharType::Number)
    }
}

// Helper for getting character type
pub fn get_char_type(ch: u8) -> CharType {
    CHAR_MAP[ch as usize]
}
