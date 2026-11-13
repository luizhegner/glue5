# With multiple game assets, select only a unique uncompressed file whose
# advertised size matches a supported build. The caller must still verify the
# complete downloaded WASM checksum. Compressed files cannot be size-matched.
[.assets[] | select(.state == "uploaded")
  | select(.name | ascii_downcase | contains("wasm") and test("\\.(wasm|zip|gz)$"))] as $all
| if ($all | length) == 1 then $all[0]
  else [$all[] | select(.name | ascii_downcase | endswith(".wasm"))
       | select(.size as $size | $sizes | index($size))] as $matches
       | if ($matches | length) == 1 then $matches[0] else empty end
  end
