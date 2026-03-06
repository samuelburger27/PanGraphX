#!/bin/bash

input_dir="${1:-src}"

if [ ! -d "$input_dir" ]; then
    echo "Error: '$input_dir' is not a directory"
    echo "Usage: $0 [directory]"
    exit 1
fi

printf "" > merged.rs
find "$input_dir" -name "*.rs" -print0 | sort -rz | while IFS= read -r -d '' file; do
    echo "// FILE: $file" >> merged.rs
    cat "$file" >> merged.rs
    echo >> merged.rs
done
