# Release Process

`happ` не выпускается поверх устаревших совместимых зависимостей.

## Правило

Перед новым тегом релиза нужно:

1. Обновить совместимые зависимости `Rust`, `Go`, `JS`.
2. Обновить встроенные pinned-зависимости `zq` и `helm-apps`, если в их текущей совместимой ветке вышла новая версия.
3. Только после этого поднимать `happ` version/tag.

Под "совместимой веткой" понимается:

1. Для `1.x.y` зависимостей проверяется latest внутри того же major.
2. Для `0.x.y` зависимостей проверяется latest внутри того же `0.x`.

Это намеренное ограничение: major-upgrade не должен попадать в релиз случайно.

## Локальная проверка

Перед релизом запусти:

```bash
./scripts/check-release-deps.sh
```

Если скрипт падает, сначала обнови зависимости, потом выпускай тег.

## CI Enforcement

Tag pipeline блокирует публикацию релиза, если `./scripts/check-release-deps.sh` находит отставание по:

1. `Cargo.toml` registry dependencies для root crate.
2. `src/go_compat/helm_ir_ffi_helper/go.mod` direct dependencies.
3. `web/package.json` direct dependencies.
4. pinned git refs `zq` и `helm-apps`.
