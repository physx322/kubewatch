//! Analyseur de flux *Server-Sent Events*, incrémental.
//!
//! Les deux fournisseurs répondent en SSE : Anthropic avec un champ `event:`
//! et un `data:` JSON, OpenAI avec de simples lignes `data:` terminées par
//! `data: [DONE]`. L'analyseur reçoit les octets par morceaux — un morceau peut
//! couper un évènement, une ligne, voire un caractère UTF-8 — et ne rend que
//! les évènements complets.

/// Évènement SSE complet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Valeur du champ `event:`, s'il y en a un.
    pub event: Option<String>,
    /// Lignes `data:` jointes par `\n`.
    pub data: String,
}

/// Analyseur incrémental.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    /// Nouvel analyseur vide.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un morceau et renvoie les évènements devenus complets.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((at, len)) = find_delimiter(&self.buffer) {
            let raw: Vec<u8> = self.buffer.drain(..at + len).collect();
            let block = String::from_utf8_lossy(&raw[..at]);
            if let Some(ev) = parse_block(&block) {
                out.push(ev);
            }
        }
        out
    }

    /// Vide ce qui reste (flux terminé sans ligne vide finale).
    pub fn finish(&mut self) -> Option<SseEvent> {
        if self.buffer.is_empty() {
            return None;
        }
        let raw = std::mem::take(&mut self.buffer);
        parse_block(&String::from_utf8_lossy(&raw))
    }
}

/// Position et longueur du premier séparateur d'évènements (`\n\n` ou `\r\n\r\n`).
fn find_delimiter(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find(buf, b"\n\n").map(|i| (i, 2));
    let crlf = find(buf, b"\r\n\r\n").map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Décode un bloc de lignes en évènement.
fn parse_block(block: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in block.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.find(':') {
            Some(i) => (
                &line[..i],
                line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..]),
            ),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if event.is_none() && data.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evenement_simple_avec_nom() {
        let mut p = SseParser::new();
        let evs = p.push(b"event: ping\ndata: {\"type\":\"ping\"}\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("ping"));
        assert_eq!(evs[0].data, "{\"type\":\"ping\"}");
    }

    #[test]
    fn donnees_multilignes_et_commentaires() {
        let mut p = SseParser::new();
        let evs = p.push(b": keep-alive\ndata: a\ndata: b\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event, None);
        assert_eq!(evs[0].data, "a\nb");
    }

    #[test]
    fn fins_de_ligne_crlf() {
        let mut p = SseParser::new();
        let evs = p.push(b"data: x\r\n\r\ndata: y\r\n\r\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].data, "x");
        assert_eq!(evs[1].data, "y");
    }

    #[test]
    fn morceaux_coupant_un_evenement_et_un_caractere() {
        let mut p = SseParser::new();
        // « é » est codé sur deux octets : on coupe entre les deux.
        let full = "data: caf\u{e9}\n\n".as_bytes().to_vec();
        let split = full.iter().position(|&b| b == 0xC3).unwrap() + 1;
        assert!(p.push(&full[..split]).is_empty());
        let evs = p.push(&full[split..]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "caf\u{e9}");
    }

    #[test]
    fn plusieurs_evenements_dans_un_morceau_et_reste() {
        let mut p = SseParser::new();
        let evs = p.push(b"data: 1\n\ndata: 2\n\ndata: 3");
        assert_eq!(evs.len(), 2);
        assert_eq!(p.finish().map(|e| e.data), Some("3".to_string()));
        assert!(p.finish().is_none());
    }

    #[test]
    fn sans_espace_apres_le_deux_points() {
        let mut p = SseParser::new();
        let evs = p.push(b"data:[DONE]\n\n");
        assert_eq!(evs[0].data, "[DONE]");
    }
}
