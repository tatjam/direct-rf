import numpy as np
import matplotlib.pyplot as plt
import sys
import pandas as pd

def plot_csv_as_bitmap(csv_file):
    """
    Plot a CSV file as a bitmap/heatmap
    """
    data = pd.read_csv(csv_file, header=None)
    array_2d = np.log10(data.values)
    
    plt.figure(figsize=(10, 8))
    
    im = plt.imshow(array_2d, 
                    cmap='viridis',
                    aspect='auto',
                    interpolation='nearest')
    
    plt.colorbar(im, label='Value')
    
    plt.xlabel('Column Index')
    plt.ylabel('Row Index')
    plt.title('2D Array Heatmap')
    
    plt.show()

if __name__ == "__main__":
    plot_csv_as_bitmap(sys.argv[1])