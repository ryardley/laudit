CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL
);

CREATE VIEW active_users AS
SELECT * FROM users WHERE name IS NOT NULL;

CREATE FUNCTION greet(name TEXT) RETURNS TEXT AS $$
BEGIN
  RETURN 'hello ' || name;
END;
$$ LANGUAGE plpgsql;

INSERT INTO users (name) VALUES ('alice');
SELECT * FROM active_users;
