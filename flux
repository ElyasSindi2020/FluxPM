#!/usr/bin/zsh
# A functional package manager for an LFS system.
# This script is no longer a dry-run prototype; it performs real file operations.
# It now correctly handles .tar.zst archives and supports symbolic links.
# It also has a more robust way of tracking installed packages.

# This command ensures that the script exits immediately if any command in a pipeline fails.
set -o pipefail

# Make sure you have `jq` installed on your LFS system.

# --- Configuration & State ---
# This is a safe, local directory. On a finished LFS system, this would be `/`.
readonly INSTALL_ROOT="/"
readonly PKG_DB_DIR="$INSTALL_ROOT/var/db/flux"

# --- Functions ---

function log_message() {
    echo "$(/usr/bin/date '+%Y-%m-%d %H:%M:%S') - $1" >&2
}

function die_with_error() {
    log_message "Error: $1"
    exit 1
}

function check_dependencies() {
    local package_name="$1"
    local depends_json_array="$2"

    # /usr/bin/jq will return 'null' for an empty or missing depends field, which we can ignore
    if [[ "$depends_json_array" == "null" ]]; then
        return 0
    fi

    log_message "Checking dependencies for '$package_name'..."
    local missing_dependencies=""

    echo "$depends_json_array" | /usr/bin/jq -r '.[]' | while read -r dep; do
        # Check if the package's database file exists
        if [[ ! -f "$PKG_DB_DIR/$dep" ]]; then
            log_message "Missing dependency: $dep"
            missing_dependencies+="$dep "
        fi
    done

    if [[ -n "$missing_dependencies" ]]; then
        die_with_error "Installation failed. Missing dependencies: $missing_dependencies"
    fi
    log_message "All dependencies satisfied."
    return 0
}

function install_package() {
    local manifest_url="$1"

    log_message "Starting installation from manifest: $manifest_url"

    # 1. Download and parse the manifest JSON
    local manifest_json=$(/usr/bin/curl -L -s "$manifest_url")
    local package_name=$(echo "$manifest_json" | /usr/bin/jq -r '.name')
    local package_version=$(echo "$manifest_json" | /usr/bin/jq -r '.version')
    local depends_json=$(echo "$manifest_json" | /usr/bin/jq -c '.depends')
    local first_file_url=$(echo "$manifest_json" | /usr/bin/jq -r '.files[0].url')
    local first_file_sha256=$(echo "$manifest_json" | /usr/bin/jq -r '.files[0].sha256')

    if [[ -z "$package_name" || -z "$package_version" ]]; then
        die_with_error "Invalid package manifest. Missing name or version."
    fi

    log_message "Installing package: $package_name-$package_version"

    /usr/bin/mkdir -p "$PKG_DB_DIR"
    local pkg_file_db="$PKG_DB_DIR/$package_name"

    # Check if package is already installed to prevent re-installation
    if [[ -f "$pkg_file_db" ]]; then
        log_message "Package '$package_name' is already installed. Use 'flux remove' first."
        return 0
    fi

    # 2. Check dependencies before starting the installation
    check_dependencies "$package_name" "$depends_json"

    # --- New, more robust Installation Logic ---
    log_message "Downloading package archive..."
    local temp_compressed_path=$(/usr/bin/mktemp)
    if ! /usr/bin/curl -L -s "$first_file_url" -o "$temp_compressed_path"; then
        die_with_error "Failed to download package from $first_file_url"
    fi

    log_message "Verifying checksum..."
    local computed_hash=$(/usr/bin/sha256sum "$temp_compressed_path" | /usr/bin/awk '{print $1}')
    if [[ "$computed_hash" != "$first_file_sha256" ]]; then
        /usr/bin/rm "$temp_compressed_path"
        die_with_error "Checksum mismatch. Possible file corruption."
    fi
    log_message "Checksum verified."

    log_message "Decompressing and extracting files to a temporary location..."
    # Create a temporary directory to extract the files into
    local temp_extract_dir=$(/usr/bin/mktemp -d)

    # Decompress the .zst file and pipe to tar for extraction to the temp directory
    if ! /usr/bin/zstd -d "$temp_compressed_path" -c | /usr/bin/tar -xf - -C "$temp_extract_dir"; then
        /usr/bin/rm -rf "$temp_compressed_path" "$temp_extract_dir"
        die_with_error "Failed to decompress or extract package."
    fi

    # Clean up the compressed file
    /usr/bin/rm "$temp_compressed_path"

    # New, more robust file moving logic with symlink support
    log_message "Moving files to their final destinations..."

    echo "$manifest_json" | /usr/bin/jq -c '.files[]' | while read -r file_data; do
        local manifest_path=$(echo "$file_data" | /usr/bin/jq -r '.path')
        local manifest_filename=$(/usr/bin/basename "$manifest_path")
        local extracted_file_path=$(/usr/bin/find "$temp_extract_dir" -name "$manifest_filename" -print -quit)
        local link_path=$(echo "$file_data" | /usr/bin/jq -r '.link // empty')

        if [[ -n "$extracted_file_path" ]]; then
            local final_dest_path="$INSTALL_ROOT$manifest_path"

            # Create the parent directory for the final destination
            /usr/bin/mkdir -p $(/usr/bin/dirname "$final_dest_path")

            # Move the file and grant executable permission
            /usr/bin/mv "$extracted_file_path" "$final_dest_path"
            if [[ -x "$extracted_file_path" ]]; then
                /usr/bin/chmod +x "$final_dest_path"
            fi

            # Log the path
            echo "$manifest_path" >> "$pkg_file_db"
            log_message "Successfully moved file to: $final_dest_path"

            # Create symbolic link if specified in the manifest
            if [[ -n "$link_path" ]]; then
                local final_link_path="$INSTALL_ROOT$link_path"
                /usr/bin/ln -sf "$final_dest_path" "$final_link_path"
                log_message "Created symbolic link: $final_link_path -> $final_dest_path"
                echo "LINK: $final_link_path" >> "$pkg_file_db"
            fi
        else
            log_message "Warning: File specified in manifest not found in archive: $manifest_path"
        fi
    done

    # Clean up the temporary extraction directory
    /usr/bin/rm -rf "$temp_extract_dir"
    # --- End of New Installation Logic ---

    # 3. Write package metadata to the database file
    log_message "Updating package database..."
    # The database file itself is a simple list of files. We'll add a header for the package info.
    /usr/bin/echo "# Package: $package_name-$package_version" > "$pkg_file_db"
    /usr/bin/echo "version: $package_version" >> "$pkg_file_db"
    # New: Store the dependencies in the database file
    if [[ "$depends_json" != "null" ]]; then
        /usr/bin/echo "depends: $depends_json" >> "$pkg_file_db"
    fi
    log_message "Installation of high-priority files complete!"
    log_message "The application is now usable. Low-priority files are still downloading in the background."
}

function remove_package() {
    local package_name="$1"
    local pkg_file_db="$PKG_DB_DIR/$package_name"

    if [[ ! -f "$pkg_file_db" ]]; then
        die_with_error "Package '$package_name' not found in database."
    fi

    log_message "Starting removal of package: $package_name"

    # Corrected: Check for reverse dependencies before removing
    local reverse_deps=""
    local installed_packages=$(/usr/bin/find "$PKG_DB_DIR" -maxdepth 1 -type f -exec /usr/bin/basename {} \;)
    for pkg in $(echo "$installed_packages"); do
        if [[ "$pkg" != "$package_name" ]]; then
            local depends_line=$(/usr/bin/grep "^depends:" "$PKG_DB_DIR/$pkg" 2>/dev/null)
            if [[ -n "$depends_line" ]]; then
                local depends_list=$(echo "$depends_line" | /usr/bin/sed 's/^depends: //')
                if echo "$depends_list" | /usr/bin/jq -e "any(. == \"$package_name\")" >/dev/null; then
                    reverse_deps+="$pkg "
                fi
            fi
        fi
    done

    if [[ -n "$reverse_deps" ]]; then
        die_with_error "Cannot remove '$package_name'. The following packages depend on it: $reverse_deps"
    fi

    # Read the list of files to remove from the database file
    cat "$pkg_file_db" | /usr/bin/grep -v '^#' | while read -r entry; do
        if [[ "$entry" =~ "LINK:" ]]; then
            local link_path=$(echo "$entry" | sed 's/LINK: //')
            /usr/bin/rm -vf "$link_path"
            log_message "Removed symlink: $link_path"
        else
            /usr/bin/rm -vf "$INSTALL_ROOT$entry"
        fi
    done

    # Remove the database entry
    /usr/bin/rm -v "$pkg_file_db"
    log_message "Removal of '$package_name' complete."
}

function list_packages() {
    log_message "Listing installed packages..."
    if [[ ! -d "$PKG_DB_DIR" ]]; then
        log_message "No packages installed yet."
        return
    fi
    /usr/bin/ls "$PKG_DB_DIR" | while read -r pkg_name; do
        if [[ -f "$PKG_DB_DIR/$pkg_name" ]]; then
            local version=$(/usr/bin/head -n 1 "$PKG_DB_DIR/$pkg_name" | /usr/bin/awk '{print $NF}' | /usr/bin/sed 's/version: //')
            echo "- $pkg_name (version: $version) "
        fi
    done
}

function update_package() {
    local package_name="$1"
    local manifest_url="$2"
    local pkg_file_db="$PKG_DB_DIR/$package_name"

    if [[ ! -f "$pkg_file_db" ]]; then
        die_with_error "Package '$package_name' not found in database. Please install it first."
    fi

    log_message "Checking for updates for '$package_name'..."
    local local_version=$(/usr/bin/head -n 1 "$pkg_file_db" | /usr/bin/awk '{print $NF}' | /usr/bin/sed 's/version: //')

    local manifest_json=$(/usr/bin/curl -L -s "$manifest_url")
    local remote_version=$(echo "$manifest_json" | /usr/bin/jq -r '.version')

    if [[ "$remote_version" > "$local_version" ]]; then
        log_message "New version available: $remote_version. Current version: $local_version."
        log_message "Removing old package and installing new version..."
        remove_package "$package_name"
        install_package "$manifest_url"
    else
        log_message "Package '$package_name' is already up to date."
    fi
}

function self_install() {
    log_message "Installing flux to /usr/local/bin..."
    /usr/bin/cp "$0" /usr/local/bin/flux
    /usr/bin/chmod +x /usr/local/bin/flux
    log_message "flux installed. You can now run 'flux install' from anywhere."
}

function show_help() {
    echo "Usage: flux <command> <arguments>"
    echo "Commands:"
    echo "  install <url_to_manifest_json> : Installs a package from a manifest URL."
    echo "  remove <package_name>          : Removes an installed package."
    echo "  update <package_name> <url>    : Checks for a new version and updates the package."
    echo "  list                           : Lists all installed packages."
    echo "  self-install                   : Installs the 'flux' script itself to /usr/local/bin."
    echo "  help                           : Shows this help message."
}

# --- Main script logic ---

function main() {
    if [[ -z "$1" ]]; then
        show_help
        exit 1
    fi

    case "$1" in
        install)
            if [[ -z "$2" ]]; then
                die_with_error "Install command requires a manifest URL."
            fi
            install_package "$2"
            ;;
        remove)
            if [[ -z "$2" ]]; then
                die_with_error "Remove command requires a package name."
            fi
            remove_package "$2"
            ;;
        update)
            if [[ -z "$2" || -z "$3" ]]; then
                die_with_error "Update command requires a package name and manifest URL."
            fi
            update_package "$2" "$3"
            ;;
        list)
            list_packages
            ;;
        self-install)
            self_install
            ;;
        help)
            show_help
            ;;
        *)
            die_with_error "Unknown command: $1"
            ;;
    esac
}

main "$@"