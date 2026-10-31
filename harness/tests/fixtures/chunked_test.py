import pytest
from sequences import chunked


@pytest.mark.parametrize("size", [0, -1])
def test_chunked_rejects_non_positive_sizes(size):
    with pytest.raises(ValueError):
        list(chunked([1, 2], size))
