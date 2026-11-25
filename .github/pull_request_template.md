## O que e por quê

<!-- Um assunto por PR. Vincule a issue: Closes #123 -->

## Testado em

<!-- Hardware e build do Windows. Em trabalho de protocolo, cole os frames reais usados. -->

## Checklist

- [ ] `cargo fmt`, `cargo clippy --all-targets -- -D warnings` e `cargo test --lib` passam
- [ ] Parsers novos têm testes com frames reais capturados
- [ ] O código de dispositivo continua somente leitura (nenhuma configuração gravada)
- [ ] Mudança visual inclui print de antes e depois
