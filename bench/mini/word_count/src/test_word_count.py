from word_count import word_count


def check(name, got, want):
    assert got == want, f"{name}: got {got!r}, want {want!r}"


check("simple", word_count("a b a"), {"a": 2, "b": 1})
check("case-insensitive keys", word_count("Apple apple APPLE"), {"apple": 3})
check("tabs and newlines", word_count("one\ttwo\none"), {"one": 2, "two": 1})
check("punctuation attached", word_count("hi, hi!"), {"hi,": 1, "hi!": 1})
check("empty", word_count(""), {})
check("whitespace only", word_count("   \n\t "), {})
print("all word_count tests passed")
