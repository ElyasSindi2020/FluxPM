import os
import tarfile
import zstandard as zstd
import hashlib
from datetime import datetime

# --- Configuration ---
# The name for our dummy package.
PACKAGE_NAME = "nginx"
# A unique version based on the current timestamp.
PACKAGE_VERSION = datetime.now().strftime("%Y%m%d%H%M%S")
# The directory to create the dummy file in.
TEMP_DIR = f"temp_{PACKAGE_NAME}"
# The name of the file to create inside the temp directory.
DUMMY_FILE_NAME = "README.txt"

# --- Main Script ---
try:
    # Step 1: Create a temporary directory and a file inside it.
    print(f"Creating temporary directory: {TEMP_DIR}")
    os.makedirs(TEMP_DIR, exist_ok=True)

    dummy_file_path = os.path.join(TEMP_DIR, DUMMY_FILE_NAME)
    with open(dummy_file_path, "w") as f:
        f.write("This is a dummy file for the nginx package.\n")
        f.write("This content should be used to generate a consistent checksum.\n")

    print(f"Created dummy file: {dummy_file_path}")

    # Step 2: Compress the directory into a .tar.zst archive.
    tar_file_name = f"{PACKAGE_NAME}-{PACKAGE_VERSION}.tar"
    zst_file_name = f"{tar_file_name}.zst"

    # Create a tar archive of the temporary directory.
    print(f"Creating tar archive: {tar_file_name}")
    with tarfile.open(tar_file_name, "w") as tar:
        tar.add(TEMP_DIR, arcname=os.path.basename(TEMP_DIR))

    # Compress the tar archive with Zstandard.
    print(f"Compressing with Zstandard: {zst_file_name}")
    cctx = zstd.ZstdCompressor()
    with open(tar_file_name, "rb") as f_in, open(zst_file_name, "wb") as f_out:
        cctx.copy_stream(f_in, f_out)

    # Step 3: Calculate the SHA256 checksum of the .tar.zst file.
    print(f"Calculating SHA256 checksum of {zst_file_name}")
    hasher = hashlib.sha256()
    with open(zst_file_name, "rb") as f:
        buf = f.read()
        hasher.update(buf)

    checksum = hasher.hexdigest()

    # Step 4: Print the results for easy copy-pasting.
    print("\n--- Results for your packages.json file ---")
    print(f"File created: {zst_file_name}")
    print(f"Checksum: {checksum}")
    print("\nYou can now use this checksum to update your packages.json file.")

except Exception as e:
    print(f"An error occurred: {e}")

finally:
    # Cleanup: remove the temporary files and directory.
    print(f"\nCleaning up temporary directory: {TEMP_DIR}")
    if os.path.exists(TEMP_DIR):
        os.system(f"rm -rf {TEMP_DIR}")
    if os.path.exists(tar_file_name):
        os.remove(tar_file_name)
