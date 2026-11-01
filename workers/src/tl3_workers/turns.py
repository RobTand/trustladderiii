"""Model turn retries and reply repairs shared by every role."""


class InvalidReply(ValueError):
    def __init__(self, first, second):
        super().__init__("reply invalid after one repair turn")
        self.errors = (first, second)
