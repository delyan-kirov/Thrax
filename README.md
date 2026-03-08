# Thrax

Low level, expression based language. 

## Build

```sh
make
```

## Raylib demo

```haskell
int black = 0
int resizeable = 4
int initial_window_size = 400
int window_title = "I'm a square"

pub main =
	let flags = set_config_flags resizeable in
	let fps = set_target_fps 60 in
	let window = init_window initial_window_size initial_window_size window_title in
	let rec_size = 100 in
	let y = 0    in let x = 0 in
	let y2 = 200 in let x2 = 200 in
	let cx1 = 0 in let cy1 = 0   in let cx2 = 200 in let cy2 = 0   in
	let cx3 = 0 in let cy3 = 200 in let cx4 = 200 in let cy4 = 200 in
	let counter = 1 in
	let random_color = 0 in
	(while !window_should_close
	=> let screan_height = get_screen_height 0
	in let counter = (counter + 1) % 10
	in let screan_width = get_screen_width 0
	in let rec_x = ((screan_width - rec_size) / 2)
	in let rec_y = ((screan_height - rec_size) / 2)
	in let y = (y + 5) % screan_height
	in let x = (x + 5) % screan_width
	in let y2 = (y2 + (screan_height - 5)) % screan_height
	in let x2 = (x2 + (screan_width - 5)) % screan_width
	in let cx1 = (cx1 + 5) % screan_width
	in let cy1 = (cy1 + 5) % screan_height
	in let cx2 = (cx2 + (screan_width - 5)) % screan_width
	in let cy2 = (cy2 + 5) % screan_height
	in let cx3 = (cx3 + 5) % screan_width
	in let cy3 = (cy3 + (screan_height - 5)) % screan_height
	in let cx4 = (cx4 + (screan_width - 5)) % screan_width
	in let cy4 = (cy4 + (screan_height - 5)) % screan_height
	in let random_color = (if !counter => -1 * (get_random_value 10 10000000) else random_color)
	in begin_drawing
	 + (clear_background black)
	 + (draw_rectangle rec_x y rec_size rec_size random_color)
	 + (draw_rectangle x rec_y rec_size rec_size random_color)
	 + (draw_rectangle rec_x y2 rec_size rec_size random_color)
	 + (draw_rectangle x2 rec_y rec_size rec_size random_color)
	 + (draw_rectangle cx1 cy1 rec_size rec_size random_color)
	 + (draw_rectangle cx2 cy2 rec_size rec_size random_color)
	 + (draw_rectangle cx3 cy3 rec_size rec_size random_color)
	 + (draw_rectangle cx4 cy4 rec_size rec_size random_color)
	 + end_drawing
	) + close_window
```

![Demo GIF](https://raw.githubusercontent.com/delyan-kirov/blobs/main/raylib-demo.gif)
