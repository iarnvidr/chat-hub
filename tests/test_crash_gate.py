from pathlib import Path


def test_exception_stack_array_is_rendered_as_description_before_utf8():
    source = (Path(__file__).parents[1] / "src-tauri/src/crash_diagnostics.rs").read_text()
    assert 'text(text(exception, b"callStackSymbols\\0"), b"description\\0")' in source
    assert 'text(exception, b"callStackSymbols\\0")' not in source
    assert source.index('write_all(initial.as_bytes())') < source.index('b"callStackSymbols\\0"')
    assert 'catch_unwind(std::panic::AssertUnwindSafe' in source
