# ---------------------------------------------------------------------------
# KubeWatch — raccourcis de développement
#
#   make            affiche cette aide
#   make build      compile le workspace en debug
#   make run        lance l'application de bureau
#   make dev        idem, recompilée et relancée à chaque sauvegarde (cargo-watch)
#   make dist       produit l'archive de distribution du poste courant
#
# Le dépôt produit un seul binaire :
#   • kubewatch-desktop  — application de bureau (egui)  (crates/desktop)
#
# TOOLCHAIN — si le shim `rustup` est cassé (cargo introuvable ou en erreur),
# exportez le chemin de la toolchain avant d'appeler make :
#
#   export PATH=~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH
#
# ou surchargez la variable à l'appel, sans rien exporter :
#
#   make build CARGO=~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo
# ---------------------------------------------------------------------------

CARGO   ?= cargo
# Nom du produit : préfixe des archives de distribution.
PKG     := kubewatch
GUI_BIN := kubewatch-desktop
APP_ID  := io.kubewatch.KubeWatch
PROFILE ?= dist
# Cible de compilation croisée ; vide = plateforme courante.
TARGET  ?=

VERSION := $(shell sed -n '/^\[workspace\.package\]/,/^\[/ s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -n1)
HOST    := $(shell rustc -vV 2>/dev/null | sed -n 's/^host: //p')
TRIPLE  := $(if $(TARGET),$(TARGET),$(if $(HOST),$(HOST),local))
TARGET_FLAG := $(if $(TARGET),--target $(TARGET),)
OUT_DIR  := target/$(if $(TARGET),$(TARGET)/,)$(PROFILE)
GUI_PATH := $(OUT_DIR)/$(GUI_BIN)

# Emplacements d'installation par utilisateur (aucun sudo nécessaire).
PREFIX      ?= $(HOME)/.local
DESKTOP_DIR := $(PREFIX)/share/applications
METAINFO_DIR := $(PREFIX)/share/metainfo

.DEFAULT_GOAL := help
.PHONY: help build release run dev test lint fmt fmt-check fmt-nightly clippy \
        audit deny dist clean install uninstall doc ci packaging-check \
        desktop-install desktop-uninstall

help: ## Affiche la liste des cibles disponibles
	@printf 'KubeWatch %s — cibles disponibles :\n\n' '$(VERSION)'
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'
	@printf '\nVariables : CARGO=%s PROFILE=%s PREFIX=%s\n' '$(CARGO)' '$(PROFILE)' '$(PREFIX)'

build: ## Compile le workspace en mode debug
	$(CARGO) build --workspace

release: ## Compile le binaire optimisé (profil dist)
	$(CARGO) build --locked --profile $(PROFILE) $(TARGET_FLAG) -p $(GUI_BIN)
	@ls -lh $(GUI_PATH)

run: ## Lance l'application de bureau (RUST_LOG=debug pour les traces)
	RUST_LOG=$${RUST_LOG:-info} $(CARGO) run -p $(GUI_BIN)

dev: ## Recompile et relance l'application à chaque sauvegarde (cargo-watch)
	@$(CARGO) watch --version >/dev/null 2>&1 \
		|| { echo "cargo-watch absent : cargo install cargo-watch --locked"; exit 1; }
	RUST_LOG=$${RUST_LOG:-info} $(CARGO) watch -w crates -w Cargo.toml -x 'run -p $(GUI_BIN)'

test: ## Exécute la suite de tests du workspace
	$(CARGO) test --workspace --all-features

lint: fmt-check clippy ## Vérifie le formatage puis exécute clippy

fmt: ## Formate tout le code
	$(CARGO) fmt --all

fmt-nightly: ## Formate en appliquant aussi le regroupement des imports (nightly)
	$(CARGO) +nightly fmt --all

fmt-check: ## Vérifie le formatage sans modifier les fichiers
	$(CARGO) fmt --all -- --check

clippy: ## Lint strict : tout avertissement est une erreur
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

doc: ## Génère la documentation du workspace
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps --all-features

audit: ## Recherche les vulnérabilités connues (cargo-audit)
	@command -v cargo-audit >/dev/null 2>&1 || { echo "cargo-audit absent : cargo install cargo-audit"; exit 1; }
	$(CARGO) audit --deny warnings

deny: ## Vérifie licences, sources et interdictions (deny.toml)
	@command -v cargo-deny >/dev/null 2>&1 || { echo "cargo-deny absent : cargo install cargo-deny"; exit 1; }
	$(CARGO) deny --all-features check

packaging-check: ## Valide l'entrée .desktop et les métadonnées AppStream
	@command -v desktop-file-validate >/dev/null 2>&1 \
		|| { echo "desktop-file-validate absent : installez desktop-file-utils"; exit 1; }
	@command -v appstreamcli >/dev/null 2>&1 \
		|| { echo "appstreamcli absent : installez appstream"; exit 1; }
	desktop-file-validate packaging/$(BIN).desktop
	appstreamcli validate --no-net packaging/$(APP_ID).metainfo.xml

ci: lint test release packaging-check audit ## Reproduit localement l'essentiel de la CI

dist: release ## Produit l'archive de distribution (dist/kubewatch-<version>-<cible>.tar.gz)
	@set -eu; \
	pkg="$(PKG)-$(VERSION)-$(TRIPLE)"; \
	rm -rf "dist/$$pkg"; \
	mkdir -p "dist/$$pkg"; \
	cp "$(GUI_PATH)" "dist/$$pkg/"; \
	cp LICENSE "dist/$$pkg/"; \
	cp packaging/$(APP_ID).desktop "dist/$$pkg/"; \
	cp packaging/$(APP_ID).metainfo.xml "dist/$$pkg/"; \
	[ -f README.md ] && cp README.md "dist/$$pkg/" || true; \
	[ -f CHANGELOG.md ] && cp CHANGELOG.md "dist/$$pkg/" || true; \
	tar -C dist -czf "dist/$$pkg.tar.gz" "$$pkg"; \
	rm -rf "dist/$$pkg"; \
	cd dist && (sha256sum "$$pkg.tar.gz" 2>/dev/null || shasum -a 256 "$$pkg.tar.gz") > "$$pkg.tar.gz.sha256"; \
	cat "$$pkg.tar.gz.sha256"

install: ## Installe le binaire dans ~/.cargo/bin
	$(CARGO) install --path crates/desktop --locked --force

uninstall: ## Désinstalle le binaire de ~/.cargo/bin
	$(CARGO) uninstall $(GUI_BIN) || true

desktop-install: ## Installe l'entrée de menu et les métadonnées AppStream (utilisateur)
	install -Dm 0644 packaging/$(APP_ID).desktop $(DESKTOP_DIR)/$(APP_ID).desktop
	install -Dm 0644 packaging/$(APP_ID).metainfo.xml $(METAINFO_DIR)/$(APP_ID).metainfo.xml
	@command -v update-desktop-database >/dev/null 2>&1 \
		&& update-desktop-database $(DESKTOP_DIR) || true
	@printf 'Entrée installée dans %s.\n' '$(DESKTOP_DIR)'
	@printf 'Rappel : « Exec=kubewatch-desktop » suppose le binaire dans le PATH.\n'

desktop-uninstall: ## Retire l'entrée de menu et les métadonnées AppStream (utilisateur)
	rm -f $(DESKTOP_DIR)/$(APP_ID).desktop $(METAINFO_DIR)/$(APP_ID).metainfo.xml
	@command -v update-desktop-database >/dev/null 2>&1 \
		&& update-desktop-database $(DESKTOP_DIR) || true

clean: ## Supprime les artefacts de compilation et de distribution
	$(CARGO) clean
	rm -rf dist
