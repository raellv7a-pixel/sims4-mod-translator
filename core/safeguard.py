import re

class TokenSafeguard:
    """Masks and unmasks Sims 4 formatting placeholders and HTML tags."""
    
    PLACEHOLDER_REGEX = re.compile(r'\{[^}]+\}')
    HTML_TAG_REGEX = re.compile(r'</?[a-zA-Z0-9]+[^>]*>')
    
    def mask(self, text):
        """Replaces placeholders and HTML tags with non-translatable tokens."""
        tokens = []
        
        def replace_ph(match):
            idx = len(tokens)
            tokens.append(match.group(0))
            return f"__SIMS_TOK_{idx}__"
            
        # First mask HTML tags, then placeholders
        masked = self.HTML_TAG_REGEX.sub(replace_ph, text)
        masked = self.PLACEHOLDER_REGEX.sub(replace_ph, masked)
        
        return masked, tokens
        
    def unmask(self, masked_text, tokens):
        """Restores original placeholders and HTML tags in translated text."""
        result = masked_text
        for i, token in enumerate(tokens):
            # Flexible pattern matching case variations and spaces added by NMT engines
            pattern = re.compile(r'\s*_*sims_?\s*tok_?\s*' + str(i) + r'_*\s*', re.IGNORECASE)
            result = pattern.sub(token, result)
            
        return result

    def preserves_tokens(self, source_text, translated_text):
        """Return whether a translation retained every protected Sims token."""
        _, source_tokens = self.mask(source_text)
        _, translated_tokens = self.mask(translated_text)
        return source_tokens == translated_tokens
