.PHONY: publish-all publish-test publish-macro publish-reactive publish-force prepare-release

# Check if a directory has changes since the last commit or tag
has_changes = $(shell git diff --quiet HEAD -- $(1) || echo "changed")

# Command to prepare Cargo.toml files for release
prepare-release:
	@echo "Preparing Cargo.toml files for publishing..."; \
	for crate in acton-test acton-macro acton-reactive; do \
		echo "Updating dependencies in $$crate/Cargo.toml..."; \
		sed -i.bak -E 's|path = "\.\./[a-zA-Z0-9_-]+"|version = "3.0.0-beta.1"|g' $$crate/Cargo.toml; \
	done

# Command to check, bump, and publish acton-test if there are changes
publish-test:
	@if [ "$(call has_changes, acton_test)" = "changed" ]; then \
		echo "Changes detected in acton-test, publishing..."; \
		cd acton_test && cargo release beta --dependent-version upgrade --no-push --quiet; \
	else \
		echo "No changes in acton-test, skipping publish."; \
	fi

# Command to check, bump, and publish acton-macro if there are changes
publish-macro: publish-test
	@if [ "$(call has_changes, acton-macro)" = "changed" ]; then \
		echo "Changes detected in acton-macro, publishing..."; \
		cd acton-macro && cargo release beta --dependent-version upgrade --no-push --quiet --execute; \
	else \
		echo "No changes in acton-macro, skipping publish."; \
	fi

# Command to check, bump, and publish acton-reactive if there are changes
publish-reactive: publish-macro
	@if [ "$(call has_changes, acton-reactive)" = "changed" ]; then \
		echo "Changes detected in acton-reactive, publishing..."; \
		cd acton-reactive && cargo release beta --dependent-version upgrade --no-push --quiet --execute; \
	else \
		echo "No changes in acton-reactive, skipping publish."; \
	fi

# Command to publish all components in the correct order
publish-all: publish-test publish-macro publish-reactive

# Command to force publish all components regardless of changes
publish-force: prepare-release
	@echo "Force publishing all components..."; \
	cd acton_test && cargo release beta --dependent-version upgrade --no-push --quiet --execute; \
	cd ../acton-macro && cargo release beta --dependent-version upgrade --no-push --quiet --execute; \
	cd ../acton-reactive && cargo release beta --dependent-version upgrade --no-push --quiet --execute;
