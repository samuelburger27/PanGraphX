printf "" > merged.rs
find src -name "*.rs" -print0 | while IFS= read -r -d '' file; do
    echo "// FILE: $file" >> merged.rs
    cat "$file" >> merged.rs
    echo >> merged.rs
done
