import sys
import urllib.request
import urllib.parse
import json
from core.safeguard import TokenSafeguard

class TranslationEngine:
    """Manager for multiple translation backends."""
    
    def __init__(self):
        self.safeguard = TokenSafeguard()
        self._init_argos()
        
    def _init_argos(self):
        self.has_argos = False
        try:
            import argostranslate.package
            import argostranslate.translate
            self.argostranslate = argostranslate
            self.has_argos = True
        except ImportError:
            pass

    def get_argos_languages(self):
        if not self.has_argos:
            return []
        try:
            langs = self.argostranslate.translate.get_installed_languages()
            return [l.code for l in langs]
        except Exception:
            return []

    def install_argos_package(self, from_code="en", to_code="pt"):
        if not self.has_argos:
            raise RuntimeError("Argos Translate library is not installed.")
            
        self.argostranslate.package.update_package_index()
        available = self.argostranslate.package.get_available_packages()
        pkg = next((p for p in available if p.from_code == from_code and p.to_code == to_code), None)
        
        if not pkg:
            raise ValueError(f"No package found for {from_code} -> {to_code}")
            
        download_path = pkg.download()
        self.argostranslate.package.install_from_path(download_path)
        return True

    def translate_argos(self, text, from_code="en", to_code="pt"):
        if not self.has_argos:
            raise RuntimeError("Argos Translate is not available.")
            
        masked_text, tokens = self.safeguard.mask(text)
        translated = self.argostranslate.translate.translate(masked_text, from_code, to_code)
        return self.safeguard.unmask(translated, tokens)

    def translate_google(self, text, from_code="en", to_code="pt"):
        """Free Google Translate web API fallback."""
        masked_text, tokens = self.safeguard.mask(text)
        
        url = "https://translate.googleapis.com/translate_a/single?client=gtx&sl=" + \
              from_code + "&tl=" + to_code + "&dt=t&q=" + urllib.parse.quote(masked_text)
              
        req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
        with urllib.request.urlopen(req, timeout=10) as response:
            result = json.loads(response.read().decode('utf-8'))
            
        translated_parts = [item[0] for item in result[0] if item[0]]
        translated = "".join(translated_parts)
        return self.safeguard.unmask(translated, tokens)

    def translate_ollama(self, text, from_code="en", to_code="pt", model="llama3", api_url="http://localhost:11434"):
        """Local Ollama AI endpoint translation."""
        masked_text, tokens = self.safeguard.mask(text)
        
        prompt = f"Translate from {from_code} to {to_code}. Keep all __SIMS_TOK_N__ markers intact. Output ONLY translation.\nText: {masked_text}"
        payload = json.dumps({"model": model, "prompt": prompt, "stream": False}).encode('utf-8')
        
        req = urllib.request.Request(f"{api_url}/api/generate", data=payload, headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=30) as response:
            result = json.loads(response.read().decode('utf-8'))
            
        translated = result.get("response", "").strip()
        return self.safeguard.unmask(translated, tokens)
