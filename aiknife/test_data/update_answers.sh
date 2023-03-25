#!/bin/bash
# Re-run the Python script that generates the expected tokens data for all input
# files in the directory.

ENCODINGS=("cl100k_base" "r50k_base" "p50k_base" "p50k_edit")

# Get the directory path where this script is located
SCRIPT_DIR=$(dirname "$(readlink -f "$0")")

# Clear the output directory and re-generate
rm -rf "${SCRIPT_DIR}/output}"

for input_file in $SCRIPT_DIR/input/*
do
    # Get the basename of the input file
    input_basename=$(basename "$input_file")

    for encoding in "${ENCODINGS[@]}"
    do
        mkdir -p "${SCRIPT_DIR}/output/${encoding}"
        output_file="$SCRIPT_DIR/output/${encoding}/$input_basename"

        echo "Generating known answers for ${input_file} using encoding ${encoding}"

        python3 "${SCRIPT_DIR}/gen_known_answers.py" "$encoding" "$input_file" > "$output_file"
    done
done

